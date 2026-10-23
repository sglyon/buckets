/** Buckets' executable-plugin SDK. The wire is JSON-RPC 2.0, never a compiler ABI. */
import type { Readable, Writable } from "node:stream";

export type RpcId = string | number;
export class RpcError extends Error {
  code: number;
  constructor(code: number, message: string) {
    super(message);
    this.code = code;
  }
}
export interface Invocation {
  protocol_version: 1;
  config: Record<string, unknown>;
  request: unknown;
}
export interface RequestContext {
  id: RpcId | undefined;
  signal: AbortSignal;
  host: { call<T = unknown>(method: string, params?: unknown): Promise<T> };
  notify(method: string, params: unknown): void;
}
export type Handler = (params: unknown, context: RequestContext) => unknown | Promise<unknown>;
export interface PluginOptions {
  handlers: Record<string, Handler>;
  input?: Readable;
  output?: Writable;
  maxFrameBytes?: number;
  lifecycle?: "auto" | "oneshot" | "service";
  shutdownTimeoutMs?: number;
  onClose?: () => void | Promise<void>;
}

/** Both legacy one-request processes and persistent duplex services use this peer. */
export async function servePlugin(options: PluginOptions): Promise<void> {
  const input = options.input ?? process.stdin;
  const output = options.output ?? process.stdout;
  const max = options.maxFrameBytes ?? 8 * 1024 * 1024;
  let sequence = 0;
  let buffer = Buffer.alloc(0);
  let ended = false;
  let service = options.lifecycle === "service";
  let fatal: Error | undefined;
  let closed = false;
  async function close(): Promise<void> {
    if (closed) return;
    closed = true;
    await options.onClose?.();
  }
  async function boundedShutdown(): Promise<void> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      await Promise.race([
        close().then(() => Promise.allSettled(inFlight)),
        new Promise<void>((resolve) => {
          timer = setTimeout(resolve, options.shutdownTimeoutMs ?? 2000);
        }),
      ]);
    } finally {
      if (timer) clearTimeout(timer);
    }
  }
  const pending = new Map<RpcId, { resolve(value: unknown): void; reject(error: Error): void }>();
  const controllers = new Map<RpcId, AbortController>();
  const inFlight = new Set<Promise<void>>();
  function send(frame: unknown): void {
    if (closed && service) return;
    const line = JSON.stringify(frame) + "\n";
    if (Buffer.byteLength(line) > max)
      throw new RpcError(-32000, "Plugin response exceeds frame limit");
    output.write(line);
  }
  function call<T>(method: string, params: unknown = {}): Promise<T> {
    if (ended) return Promise.reject(new RpcError(-32000, "Host connection closed"));
    const id = `plugin-${++sequence}`;
    return new Promise<T>((resolve, reject) => {
      pending.set(id, { resolve: (value) => resolve(value as T), reject });
      try {
        send({ jsonrpc: "2.0", id, method, params });
      } catch (error) {
        pending.delete(id);
        reject(error);
      }
    });
  }
  async function dispatch(frame: unknown): Promise<void> {
    if (!frame || typeof frame !== "object") throw new RpcError(-32600, "Invalid JSON-RPC frame");
    const value = frame as Record<string, unknown>;
    if (value.jsonrpc !== "2.0") throw new RpcError(-32600, "Invalid JSON-RPC version");
    const id = value.id;
    const hasId = typeof id === "string" || (typeof id === "number" && Number.isSafeInteger(id));
    if (typeof value.method === "string" && ("result" in value || "error" in value))
      throw new RpcError(-32600, "Ambiguous JSON-RPC frame");
    if (value.id !== undefined && !hasId) throw new RpcError(-32600, "Invalid JSON-RPC request ID");
    if (typeof value.method !== "string") {
      if (!hasId || !pending.has(id as RpcId) || "result" in value === "error" in value)
        throw new RpcError(-32600, "Unexpected JSON-RPC response");
      const task = pending.get(id as RpcId)!;
      pending.delete(id as RpcId);
      if ("error" in value) {
        const error = value.error as { code?: number; message?: string };
        task.reject(new RpcError(error?.code ?? -32000, error?.message ?? "Host operation failed"));
      } else task.resolve(value.result);
      return;
    }
    if (value.method === "$/cancelRequest") {
      const target = (value.params as { id?: RpcId } | undefined)?.id;
      if (target !== undefined) controllers.get(target)?.abort();
      return;
    }
    if (
      ["agent.invoke", "accounts.invoke"].includes(value.method) &&
      options.lifecycle !== "oneshot"
    )
      service = true;
    if (hasId && controllers.has(id as RpcId))
      throw new RpcError(-32600, "Duplicate in-flight request ID");
    const controller = new AbortController();
    if (hasId) controllers.set(id as RpcId, controller);
    const handler = options.handlers[value.method];
    try {
      if (!handler) throw new RpcError(-32601, `Unknown method: ${value.method}`);
      const result = await handler(value.params, {
        id: hasId ? (id as RpcId) : undefined,
        signal: controller.signal,
        host: { call },
        notify: (method, params) => send({ jsonrpc: "2.0", method, params }),
      });
      if (hasId) send({ jsonrpc: "2.0", id, result: result ?? null });
    } catch (error) {
      if (hasId)
        send({
          jsonrpc: "2.0",
          id,
          error: {
            code: error instanceof RpcError ? error.code : -32000,
            message: error instanceof Error ? error.message : "Plugin invocation failed",
          },
        });
    } finally {
      if (hasId) controllers.delete(id as RpcId);
    }
  }
  try {
    for await (const chunk of input) {
      buffer = Buffer.concat([buffer, Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)]);
      while (true) {
        const newline = buffer.indexOf(10);
        if (newline < 0) break;
        if (newline > max) throw new RpcError(-32000, "Host frame exceeds frame limit");
        const line = buffer.subarray(0, newline).toString("utf8");
        buffer = buffer.subarray(newline + 1);
        if (!line.trim()) throw new RpcError(-32700, "Empty JSON-RPC frame");
        let frame: unknown;
        try {
          frame = JSON.parse(line);
        } catch {
          throw new RpcError(-32700, "Invalid JSON-RPC JSON");
        }
        const work = dispatch(frame);
        inFlight.add(work);
        void work
          .catch((error) => {
            fatal = error instanceof Error ? error : new Error("Invalid plugin frame");
            input.destroy(fatal);
          })
          .finally(() => inFlight.delete(work))
          .catch(() => {});
      }
      if (buffer.length > max) throw new RpcError(-32000, "Host frame exceeds frame limit");
    }
    if (buffer.length) throw new RpcError(-32700, "Host frame must end with a newline");
    // Legacy hosts close stdin immediately after sending their one request.
    // A duplex service instead closes only during shutdown. Outstanding callbacks
    // cannot complete after EOF; reject them, then let request handlers settle.
    ended = true;
    for (const task of pending.values())
      task.reject(new RpcError(-32000, "Host connection closed"));
    pending.clear();
    if (service) {
      for (const controller of controllers.values())
        controller.abort(new RpcError(-32098, "Host connection closed"));
      await boundedShutdown();
    } else await Promise.all(inFlight);
    if (fatal) throw fatal;
  } finally {
    ended = true;
    for (const controller of controllers.values())
      controller.abort(new RpcError(-32098, "Host connection closed"));
    for (const task of pending.values())
      task.reject(new RpcError(-32000, "Host connection closed"));
    if (service) await boundedShutdown();
    else await close();
  }
}

export interface WorkspaceContext {
  workspace_id: string;
  revision: number;
  panes: {
    id: string;
    location: string;
    selected: string[];
    cursor?: string | null;
    entries: { location: string; name: string; is_dir: boolean }[];
    capabilities: string[];
  }[];
}
export interface AgentRequest {
  request_id: string;
  mode: "ask" | "search" | "memory" | "index";
  prompt: string;
  context: WorkspaceContext;
  storage_dir: string;
  memory_update?: string;
  cancelled_request_ids?: string[];
}
export interface Citation {
  location: string;
  line?: number;
  label?: string;
}
export interface AgentFile {
  location: string;
  name: string;
  is_dir?: boolean;
  line?: number;
  snippet?: string;
  reason?: string;
  score?: number;
}
export interface AgentResult {
  answer: string;
  citations: Citation[];
  results: AgentFile[];
  memory?: string;
}
/** Full replaceable provisional snapshots. Only the final result carries verified file actions. */
export type AgentPhase = "waiting" | "reading" | "responding" | "verifying";
export interface AgentActivity {
  id: string;
  title: string;
  state: "running" | "complete" | "error";
}
export interface AgentUpdate {
  sequence: number;
  text: string;
  phase: AgentPhase;
  activities: AgentActivity[];
}
export interface AgentProgress {
  request_id: string;
  message: string;
  update?: AgentUpdate;
}
export interface FileRead {
  location: string;
  name?: string;
  text: string;
  truncated: boolean;
  binary: boolean;
}
export interface FileSearch {
  results: AgentFile[];
  truncated: boolean;
}

/** Account management is separate from model tools and workspace checkpoints. */
export interface AccountRequest {
  action: "status" | "sign_in" | "sign_out" | "select" | "models";
  account_id?: string;
}
export interface AccountInfo {
  id: string;
  label: string;
  connected: boolean;
  plan_enabled: boolean;
}
export interface AccountModel {
  id: string;
  name: string;
}
export interface AccountState {
  accounts: AccountInfo[];
  active_account_id?: string;
  models: AccountModel[];
  message?: string;
  plan_enabled: boolean;
  first_connection: boolean;
  usage_url?: string;
  using_plan: boolean;
  sign_in_required: boolean;
  sign_in_label?: string;
  plan_label?: string;
}

/** Static, language-independent command metadata; listing must not probe paths. */

export type CommandLocation = "local" | "ssh" | "s3" | "git" | "plugin";
export type CommandArgumentKind = "text" | "path" | "directory" | "choice";
export interface CommandArgument {
  name: string;
  kind?: CommandArgumentKind;
  required?: boolean;
  variadic?: boolean;
  choices?: string[];
}
export interface CommandLineSpec {
  name: string;
  aliases?: string[];
  arguments?: CommandArgument[];
  /** Empty/omitted accepts every provider; never performs a network probe. */
  locations?: CommandLocation[];
  unavailable_reason?: string;
}
export interface CommandDescriptor {
  id: string;
  title: string;
  description?: string;
  shortcut?: string;
  command_line?: CommandLineSpec;
}
export interface CommandArguments {
  /** A frozen location and selection from the pane that submitted the request. */
  location: string;
  selected: string[];
  /** Absent for legacy palette/shortcut callers. Never join/reparse as shell. */
  user_args?: string[];
}
export interface TerminalLaunchRequest {
  program: string;
  args?: string[];
  cwd: string;
  title: string;
  env?: Record<string, string>;
}
export interface CommandRegistration {
  descriptor: CommandDescriptor;
  run(arguments_: CommandArguments, context: RequestContext): unknown | Promise<unknown>;
}

const byteLength = (value: string) => Buffer.byteLength(value, "utf8");
function strings(
  value: unknown,
  maxCount: number,
  maxBytes: number,
  total: number,
): value is string[] {
  return (
    Array.isArray(value) &&
    value.length <= maxCount &&
    value.every(
      (item) => typeof item === "string" && !item.includes("\0") && byteLength(item) <= maxBytes,
    ) &&
    value.reduce((sum: number, item: string) => sum + byteLength(item), 0) <= total
  );
}

/** Convenience handlers over the unchanged v1 envelope. The Rust host validates
 * descriptor metadata, permissions, provider availability and typed host actions.
 * Generic servePlugin handlers remain available for legacy/custom commands.
 */
export function commandHandlers(
  registrations: readonly CommandRegistration[],
): Record<string, Handler> {
  const commands = new Map<string, CommandRegistration>();
  for (const entry of registrations) {
    if (commands.has(entry.descriptor.id)) throw new RpcError(-32602, "Duplicate command ID");
    commands.set(entry.descriptor.id, entry);
  }
  return {
    "commands.list": () => registrations.map((entry) => entry.descriptor),
    "commands.invoke": async (params, context) => {
      const envelope = params as Invocation | undefined;
      const request = envelope?.request as { command?: unknown; arguments?: unknown } | undefined;
      const command =
        typeof request?.command === "string" ? commands.get(request.command) : undefined;
      if (!command) throw new RpcError(-32602, "Unknown command ID");
      const value = request?.arguments as Partial<CommandArguments> | undefined;
      if (
        !value ||
        typeof value.location !== "string" ||
        !value.location ||
        value.location.includes("\0") ||
        byteLength(value.location) > 32768 ||
        !strings(value.selected, 10000, 32768, 524288) ||
        value.selected.some((item) => !item) ||
        (value.user_args !== undefined && !strings(value.user_args, 256, 32768, 131072))
      )
        throw new RpcError(-32602, "Invalid frozen command arguments");
      const argv = value.user_args;
      const spec = command.descriptor.command_line;
      if (argv !== undefined && spec) {
        const definitions = spec.arguments ?? [];
        const required = definitions.filter((arg) => arg.required).length;
        if (
          argv.length < required ||
          (!definitions.at(-1)?.variadic && argv.length > definitions.length)
        )
          throw new RpcError(-32602, "Invalid command argument count");
        for (let i = 0; i < argv.length; i++) {
          const definition = definitions[i] ?? definitions.at(-1);
          if (definition?.kind === "choice" && !definition.choices?.includes(argv[i]))
            throw new RpcError(-32602, `Invalid value for argument ${definition.name}`);
        }
      }
      if (context.signal.aborted) throw new RpcError(-32098, "Command cancelled");
      return command.run(
        {
          location: value.location,
          selected: [...value.selected],
          ...(argv === undefined ? {} : { user_args: [...argv] }),
        },
        context,
      );
    },
  };
}
