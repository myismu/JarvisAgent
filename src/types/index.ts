export interface JarvisResult {
  status: string;
  content: string;
  input_tokens: number;
  output_tokens: number;
}

export interface TodoItem {
  id: string;
  text: string;
  status: "pending" | "in_progress" | "completed";
}

export interface PermissionRequest {
  id: string;
  message: string;
}

// 方案审批数据结构
export interface PlanProposal {
  id: string;
  title: string;
  content: string;
}
