//! # db/mod.rs — SQLite 连接、只读连接池与事务入口
//!
//! 负责初始化本地 SQLite 数据库（WAL 模式）、维护写连接与固定容量只读连接池，
//! 并提供同步与 `spawn_blocking` 异步包装接口，不承载任何 `core` 业务领域模型。
//!
//! ## Key Exports
//! - `init()`: 初始化写连接、建表迁移与 WAL 只读连接池
//! - `with_connection()` / `with_connection_async()`: 以互斥写连接执行写操作
//! - `with_read_connection()` / `with_read_connection_async()`: 从只读连接池并发执行纯读查询
//! - `with_transaction()` / `with_transaction_async()`: 以串行写事务执行原子更新
//! - `is_available()`: 判断数据库是否已完成初始化
//! - `load_attachment_bytes()`: 按附件文件名读取 `session_attachments` 表的 `(media_type, bytes)`
//!
//! ## Dependencies
//! - Internal: `crate::infra::config::data_paths`, `crate::infra::db::schema`
//! - External: `rusqlite`, `tokio`
//!
//! ## Constraints
//! - 写路径通过全局 `DB` Mutex 串行化，避免 SQLite 多写者锁冲突
//! - 读路径通过 `READ_POOL`（`SQLITE_OPEN_READ_ONLY` + `PRAGMA query_only = ON`）在 WAL 下并发读取，
//!   不被长写事务阻塞；在未初始化只读池的特殊场景自动回退到主连接

pub mod schema;

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction};
use std::path::Path;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

/// 只读连接池默认容量（覆盖主窗口消息分页、列表刷新、监控面板与附件并发读取）。
const READ_POOL_SIZE: usize = 4;

/// SQLite 遇写锁/检查点竞争时的最长等待时间（毫秒）。
const SQLITE_BUSY_TIMEOUT_MS: u64 = 5000;

/// 全局唯一写连接（串行化写操作与事务，避免多写者互相 `SQLITE_BUSY`）。
static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// WAL 模式下的只读连接池（仅允许 `SELECT`，与写连接并发执行）。
static READ_POOL: OnceLock<ReadConnectionPool> = OnceLock::new();

/// 轻量级固定容量只读连接池（零外部依赖，类似 Java `BlockingQueue<Connection>` 语义）。
struct ReadConnectionPool {
    idle: Mutex<Vec<Connection>>,
    available: Condvar,
}

/// RAII 借用守卫：离开作用域（无论正常返回、`Err` 还是 `panic` 展开）自动归还连接到池中。
struct PooledReadConnection<'a> {
    pool: &'a ReadConnectionPool,
    conn: Option<Connection>,
}

impl PooledReadConnection<'_> {
    fn get(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("只读连接在借用生命周期内必然有效")
    }
}

impl Drop for PooledReadConnection<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            let mut idle = self.pool.idle.lock().unwrap_or_else(|e| e.into_inner());
            idle.push(conn);
            // 唤醒一个正在等待空闲只读连接的线程
            self.pool.available.notify_one();
        }
    }
}

impl ReadConnectionPool {
    fn open(path: &Path, size: usize) -> Result<Self, String> {
        let count = size.max(1);
        let mut conns = Vec::with_capacity(count);
        for _ in 0..count {
            let conn = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|e| format!("打开 SQLite 只读连接失败: {}", e))?;
            configure_read_connection(&conn)?;
            conns.push(conn);
        }
        Ok(Self {
            idle: Mutex::new(conns),
            available: Condvar::new(),
        })
    }

    fn acquire(&self) -> PooledReadConnection<'_> {
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(conn) = idle.pop() {
                return PooledReadConnection {
                    pool: self,
                    conn: Some(conn),
                };
            }
            // 所有只读连接均在忙时阻塞等待归还
            idle = self.available.wait(idle).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn with_connection<T, F>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&Connection) -> Result<T, String>,
    {
        let guard = self.acquire();
        f(guard.get())
    }
}

pub fn init() -> Result<(), String> {
    if DB.get().is_some() {
        return Ok(());
    }

    let path = crate::infra::config::data_paths::sqlite_db_path();
    let conn = Connection::open(&path).map_err(|e| e.to_string())?;
    configure_connection(&conn)?;
    schema::init_schema(&conn)?;

    // 必须在写连接完成 WAL 切换和建表迁移之后再打开只读连接池
    let read_pool = ReadConnectionPool::open(&path, READ_POOL_SIZE)?;

    let _ = DB.set(Mutex::new(conn));
    let _ = READ_POOL.set(read_pool);
    Ok(())
}

pub fn is_available() -> bool {
    DB.get().is_some()
}

pub fn with_connection<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&Connection) -> Result<T, String>,
{
    let db = DB
        .get()
        .ok_or_else(|| "SQLite 数据库尚未初始化".to_string())?;
    let conn = db.lock().map_err(|_| "SQLite 数据库锁已损坏".to_string())?;
    f(&conn)
}

/// 从只读连接池借出连接执行纯 `SELECT` 查询，不与写连接争抢互斥锁。
///
/// 若在特殊启动/测试路径下只读池尚未初始化，自动回退到主连接 `with_connection`。
/// 严禁在 `with_transaction` 闭包内部再调用本函数去读同一事务尚未提交的数据。
pub fn with_read_connection<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&Connection) -> Result<T, String>,
{
    if let Some(pool) = READ_POOL.get() {
        return pool.with_connection(f);
    }
    with_connection(f)
}

pub fn with_transaction<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&Transaction<'_>) -> Result<T, String>,
{
    let db = DB
        .get()
        .ok_or_else(|| "SQLite 数据库尚未初始化".to_string())?;
    let mut conn = db.lock().map_err(|_| "SQLite 数据库锁已损坏".to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let result = f(&tx)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(result)
}

/// 在 Tokio 阻塞线程池（`spawn_blocking`）中执行只读 SQLite 查询，避免占用异步工作线程。
pub async fn with_read_connection_async<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(move || with_read_connection(f))
        .await
        .map_err(|e| format!("SQLite 异步只读任务执行失败: {}", e))?
}

/// 在 Tokio 阻塞线程池（`spawn_blocking`）中执行 SQLite 写连接操作。
pub async fn with_connection_async<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(move || with_connection(f))
        .await
        .map_err(|e| format!("SQLite 异步写入任务执行失败: {}", e))?
}

/// 在 Tokio 阻塞线程池（`spawn_blocking`）中执行 SQLite 写事务。
pub async fn with_transaction_async<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&Transaction<'_>) -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(move || with_transaction(f))
        .await
        .map_err(|e| format!("SQLite 异步事务任务执行失败: {}", e))?
}

/// 读取 `session_attachments` 中存储的附件原始字节 `(media_type, bytes)`。
///
/// 放在 `infra::db` 供底层 LLM 适配器（`infra::llm::adapters`）与上层资源仓储共用，
/// 避免 `infra` 反向依赖 `core::session`。
pub fn load_attachment_bytes(filename: &str) -> Result<Option<(String, Vec<u8>)>, String> {
    let base = Path::new(filename)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| filename.to_string());
    with_read_connection(|conn| {
        conn.query_row(
            "SELECT media_type, data FROM session_attachments WHERE filename = ?1",
            [base],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

fn configure_connection(conn: &Connection) -> Result<(), String> {
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn configure_read_connection(conn: &Connection) -> Result<(), String> {
    // 内核级只读护栏：任何误发的写语句都会立刻报错，不会触碰写锁
    conn.pragma_update(None, "query_only", "ON")
        .map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod read_pool_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    fn create_wal_test_db() -> (tempfile::TempDir, Connection, ReadConnectionPool) {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let db_path = dir.path().join("wal_pool_test.sqlite3");
        let writer = Connection::open(&db_path).expect("打开写连接");
        configure_connection(&writer).expect("配置 WAL");
        writer
            .execute_batch(
                "CREATE TABLE kv (id INTEGER PRIMARY KEY, val INTEGER NOT NULL);
                 INSERT INTO kv(id, val) VALUES (1, 0);",
            )
            .expect("初始化测试表");
        let pool = ReadConnectionPool::open(&db_path, READ_POOL_SIZE).expect("打开只读连接池");
        (dir, writer, pool)
    }

    #[test]
    fn wal_read_pool_serves_concurrent_readers_alongside_writer() {
        let (_dir, writer, pool) = create_wal_test_db();
        let pool = Arc::new(pool);
        let done = Arc::new(AtomicBool::new(false));
        let total_writes = 40i64;

        // 启动 8 个并发读线程（大于连接池容量 4，同时验证 Condvar 排队与 WAL 读写并发）
        let mut readers = Vec::new();
        for _ in 0..8 {
            let pool = Arc::clone(&pool);
            let done = Arc::clone(&done);
            readers.push(std::thread::spawn(move || {
                let mut reads = 0usize;
                let mut last_seen = 0i64;
                while !done.load(Ordering::Acquire) || reads < 5 {
                    let val: i64 = pool
                        .with_connection(|conn| {
                            conn.query_row("SELECT val FROM kv WHERE id = 1", [], |r| r.get(0))
                                .map_err(|e| e.to_string())
                        })
                        .expect("WAL 并发读不应报 SQLITE_BUSY 或失败");
                    assert!(val >= last_seen, "WAL 读值不应回退: {val} < {last_seen}");
                    last_seen = val;
                    reads += 1;
                }
                reads
            }));
        }

        // 单写线程持续事务提交
        for step in 1..=total_writes {
            writer
                .execute("UPDATE kv SET val = ?1 WHERE id = 1", [step])
                .expect("写入更新失败");
            std::thread::yield_now();
        }
        done.store(true, Ordering::Release);

        for handle in readers {
            let reads = handle.join().expect("读线程不应 panic");
            assert!(reads >= 5);
        }

        // 写事务提交后，只读池应立即看到最新值
        let latest: i64 = pool
            .with_connection(|conn| {
                conn.query_row("SELECT val FROM kv WHERE id = 1", [], |r| r.get(0))
                    .map_err(|e| e.to_string())
            })
            .expect("读取最终值");
        assert_eq!(latest, total_writes);
    }

    #[test]
    fn read_pool_enforces_query_only_and_recovers_after_error_and_panic() {
        let (_dir, _writer, pool) = create_wal_test_db();

        // 1. 只读连接必须拒绝写入语句
        let write_err = pool.with_connection(|conn| {
            conn.execute("INSERT INTO kv(id, val) VALUES (2, 99)", [])
                .map_err(|e| e.to_string())
        });
        assert!(write_err.is_err(), "PRAGMA query_only=ON 应拒绝写入");

        // 2. 即使连续多次在闭包内 panic，RAII Drop 也必须把连接全部归还（不泄漏、不死锁）
        for _ in 0..(READ_POOL_SIZE * 2) {
            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = pool.with_connection(|_conn| -> Result<(), String> {
                    panic!("模拟读闭包异常");
                });
            }));
            assert!(panicked.is_err());
        }

        // 3. 池中全部连接完好无损，仍可正常读取
        let idle_count = pool.idle.lock().unwrap().len();
        assert_eq!(idle_count, READ_POOL_SIZE);
        let val: i64 = pool
            .with_connection(|conn| {
                conn.query_row("SELECT val FROM kv WHERE id = 1", [], |r| r.get(0))
                    .map_err(|e| e.to_string())
            })
            .expect("异常恢复后应可继续读取");
        assert_eq!(val, 0);
    }
}
