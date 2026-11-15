// Typed calls of the hame web API over a transport.

import type { Transport } from "../transport";
import type { ArtifactRef, ModeName, ServerInfo, SessionDetail, SessionSummary } from "../model/types";

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
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

  create(body: { cwd?: string; mode?: ModeName; provider?: string; model?: string }) {
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

  settings(id: string, body: { mode?: ModeName; provider?: string; model?: string }) {
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

  approve(id: string, approval: number, allow: boolean, always = false) {
    return this.call<object>("POST", `/api/sessions/${id}/approval`, { id: approval, allow, always });
  }

  async upload(id: string, file: File): Promise<Upload> {
    const query = new URLSearchParams({ name: file.name || "attachment" });
    const response = await this.transport.upload(`/api/sessions/${id}/attachments?${query}`, file, file.type);
    if (!response.ok) throw new ApiError(response.status, (await response.text()) || "upload failed");
    return (await response.json()) as Upload;
  }
}
