import { invoke } from "@tauri-apps/api/core";
import type {
  SnapshotTreeView,
  SnapshotSummary,
  Snapshot,
  Patch,
  AgentSandbox,
  SandboxComparison,
  MergeResult,
  Conflict,
  ConflictResolution,
} from "../types";

export class SnapshotTimelineService {
  private sessionId: string;
  private treeCache: SnapshotTreeView | null = null;
  private summaryCache = new Map<string, SnapshotSummary>();
  private detailCache = new Map<string, Snapshot>();

  constructor(sessionId: string) {
    this.sessionId = sessionId;
  }

  async loadTree(): Promise<SnapshotTreeView> {
    if (!this.treeCache) {
      this.treeCache = await invoke<SnapshotTreeView>("snapshot_get_tree_view", {
        sessionId: this.sessionId,
      });
    }
    return this.treeCache;
  }

  async loadSummaries(ids: string[]): Promise<SnapshotSummary[]> {
    const uncached = ids.filter((id) => !this.summaryCache.has(id));
    if (uncached.length > 0) {
      const summaries = await invoke<SnapshotSummary[]>(
        "snapshot_get_summaries",
        {
          sessionId: this.sessionId,
          snapshotIds: uncached,
        }
      );
      summaries.forEach((s) => this.summaryCache.set(s.id, s));
    }
    return ids.map((id) => this.summaryCache.get(id)!).filter(Boolean);
  }

  async loadDetail(id: string): Promise<Snapshot | null> {
    if (!this.detailCache.has(id)) {
      const detail = await invoke<Snapshot | null>("snapshot_get_detail", {
        sessionId: this.sessionId,
        snapshotId: id,
      });
      if (detail) {
        this.detailCache.set(id, detail);
      }
      return detail;
    }
    return this.detailCache.get(id) || null;
  }

  async createSnapshot(
    patches: Patch[],
    message?: string,
    agentId?: string,
    workspaceId?: string
  ): Promise<Snapshot> {
    const snapshot = await invoke<Snapshot>("snapshot_create", {
      sessionId: this.sessionId,
      patches,
      message,
      agentId,
      workspaceId,
    });
    this.treeCache = null;
    this.detailCache.set(snapshot.id, snapshot);
    return snapshot;
  }

  async createBranch(
    branchName: string,
    fromSnapshotId?: string,
    agentId?: string,
    description?: string
  ): Promise<void> {
    await invoke("snapshot_create_branch", {
      sessionId: this.sessionId,
      branchName,
      fromSnapshotId,
      agentId,
      description,
    });
    this.treeCache = null;
  }

  async switchBranch(branchName: string): Promise<void> {
    await invoke("snapshot_switch_branch", {
      sessionId: this.sessionId,
      branchName,
    });
    this.treeCache = null;
  }

  async rollback(snapshotId: string, targetDir: string): Promise<void> {
    await invoke("snapshot_rollback", {
      sessionId: this.sessionId,
      snapshotId,
      targetDir,
    });
    this.treeCache = null;
  }

  async getCurrent(): Promise<{ branch: string; snapshotId: string }> {
    const [branch, snapshotId] = await invoke<[string, string]>(
      "snapshot_get_current",
      {
        sessionId: this.sessionId,
      }
    );
    return { branch, snapshotId };
  }

  clearCache(): void {
    this.treeCache = null;
    this.summaryCache.clear();
    this.detailCache.clear();
  }

  // === P6: 多Agent沙箱方法 ===

  async createSandbox(
    agentId: string,
    baseSnapshotId: string,
    description?: string
  ): Promise<AgentSandbox> {
    const sandbox = await invoke<AgentSandbox>("sandbox_create", {
      sessionId: this.sessionId,
      agentId,
      baseSnapshotId,
      description,
    });
    this.treeCache = null;
    return sandbox;
  }

  async getSandbox(sandboxId: string): Promise<AgentSandbox | null> {
    return invoke<AgentSandbox | null>("sandbox_get", {
      sessionId: this.sessionId,
      sandboxId,
    });
  }

  async listSandboxes(): Promise<AgentSandbox[]> {
    return invoke<AgentSandbox[]>("sandbox_list", {
      sessionId: this.sessionId,
    });
  }

  async completeSandbox(sandboxId: string): Promise<void> {
    await invoke("sandbox_complete", {
      sessionId: this.sessionId,
      sandboxId,
    });
  }

  async abandonSandbox(sandboxId: string): Promise<void> {
    await invoke("sandbox_abandon", {
      sessionId: this.sessionId,
      sandboxId,
    });
    this.treeCache = null;
  }

  async publishSandbox(sandboxId: string): Promise<string> {
    const result = await invoke<string>("sandbox_publish", {
      sessionId: this.sessionId,
      sandboxId,
    });
    this.treeCache = null;
    return result;
  }

  async compareSandboxes(): Promise<SandboxComparison[]> {
    return invoke<SandboxComparison[]>("sandbox_compare", {
      sessionId: this.sessionId,
    });
  }

  // === P7: 分支合并方法 ===

  async previewMerge(
    sourceBranch: string,
    targetBranch: string
  ): Promise<MergeResult> {
    return invoke<MergeResult>("merge_preview", {
      sessionId: this.sessionId,
      sourceBranch,
      targetBranch,
    });
  }

  async getMergeConflicts(
    sourceBranch: string,
    targetBranch: string
  ): Promise<Conflict[]> {
    return invoke<Conflict[]>("merge_get_conflicts", {
      sessionId: this.sessionId,
      sourceBranch,
      targetBranch,
    });
  }

  async executeMerge(
    sourceBranch: string,
    targetBranch: string,
    resolutions: Record<string, ConflictResolution>,
    message?: string
  ): Promise<Snapshot> {
    const snapshot = await invoke<Snapshot>("merge_execute", {
      sessionId: this.sessionId,
      sourceBranch,
      targetBranch,
      resolutions,
      message,
    });
    this.treeCache = null;
    return snapshot;
  }
}

function collectAllSnapshotIds(node: { id: string; children: any[] }): string[] {
  const ids = [node.id];
  if (node.children) {
    for (const child of node.children) {
      ids.push(...collectAllSnapshotIds(child));
    }
  }
  return ids.filter((id) => id && id.length > 0);
}

export function collectAllIdsFromTree(tree: SnapshotTreeView): string[] {
  const ids: string[] = [];
  for (const branch of tree.branches) {
    ids.push(...collectAllSnapshotIds(branch.root));
  }
  return ids;
}
