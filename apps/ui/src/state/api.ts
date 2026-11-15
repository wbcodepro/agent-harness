// Typed calls of the hame web API over a transport.

import type { Transport } from "../transport";
import type { ApprovalEvent, ArtifactRef, ModeName, ServerInfo, SessionDetail, SessionSummary } from "../model/types";

export class ApiError extends Error {
  /** The response body, when it was JSON. */
  readonly body: Record<string, unknown> | null;

  constructor(
    readonly status: number,
    text: string,
  ) {
    let body: Record<string, unknown> | null = null;
    try {
      const parsed: unknown = JSON.parse(text);
      if (parsed && typeof parsed === "object") body = parsed as Record<string, unknown>;
    } catch {
      // A plain-text message.
    }
    super(typeof body?.message === "string" ? body.message : text);
    this.body = body;
  }

  /** The nonce of a 428 answer, to send back to confirm "Full access". */
  get confirm(): string | null {
    return this.status === 428 && typeof this.body?.confirm === "string" ? this.body.confirm : null;
  }
}

export interface Upload {
  kind: "image" | "file";
  artifact?: ArtifactRef;
  path?: string;
  bytes?: number;
}

export class Api {
  constructor(readonly transport: Transport) {}

  private async call<T>(method: string, path: string, body?: unknown): Promise<T> {
    const response = await this.transport.request(method, path, body);
    if (!response.ok) {
      const text = await response.text().catch(() => "");
      throw new ApiError(response.status, text || `request failed (${response.status})`);
    }
    return (await response.json()) as T;
  }

  info() {
    return this.call<ServerInfo>("GET", "/api/info");
  }

  sessions() {
    return this.call<SessionSummary[]>("GET", "/api/sessions");
  }

  create(body: { cwd?: string; mode?: ModeName; provider?: string; model?: string; confirm?: string }) {
    return this.call<{ id: string }>("POST", "/api/sessions", body);
  }

  detail(id: string) {
    return this.call<SessionDetail>("GET", `/api/sessions/${id}`);
  }

  rename(id: string, body: { title?: string | null; pinned?: boolean }) {
    return this.call<SessionSummary>("PATCH", `/api/sessions/${id}`, body);
  }

  remove(id: string) {
    return this.call<object>("DELETE", `/api/sessions/${id}`);
  }

  settings(id: string, body: { mode?: ModeName; provider?: string; model?: string; confirm?: string }) {
    return this.call<{ mode: ModeName; provider: string | null; model: string | null }>("PUT", `/api/sessions/${id}/settings`, body);
  }

  prompt(id: string, body: { text: string; images?: string[]; files?: string[] }) {
    return this.call<{ started: boolean }>("POST", `/api/sessions/${id}/prompt`, body);
  }

  steer(id: string, text: string) {
    return this.call<{ queued: string[] }>("POST", `/api/sessions/${id}/steer`, { text });
  }

  withdraw(id: string, index: number, text: string) {
    return this.call<{ queued: string[] }>("POST", `/api/sessions/${id}/steer/withdraw`, { index, text });
  }

  cancel(id: string) {
    return this.call<object>("POST", `/api/sessions/${id}/cancel`, {});
  }

  /** Answers `approval`, naming its tool call and arguments as the host requires. */
  approve(id: string, approval: ApprovalEvent, allow: boolean, always = false) {
    return this.call<object>("POST", `/api/sessions/${id}/approval`, {
      id: approval.id,
      allow,
      always,
      tool_call_id: approval.tool_call_id ?? "",
      args_sha256: approval.args_sha256,
    });
  }

  revokeGrant(id: string, tool: string) {
    return this.call<{ grants: string[] }>("DELETE", `/api/sessions/${id}/grants/${encodeURIComponent(tool)}`);
  }

  async upload(id: string, file: File): Promise<Upload> {
    const query = new URLSearchParams({ name: file.name || "attachment" });
    const response = await this.transport.upload(`/api/sessions/${id}/attachments?${query}`, file, file.type);
    if (!response.ok) throw new ApiError(response.status, (await response.text()) || "upload failed");
    return (await response.json()) as Upload;
  }
}
