/** Public streaming is an observation of Pi's durable event stream. It never
 * publishes reasoning blocks, tool output, raw JSON, or file navigation claims. */
import { createHash } from "node:crypto";
import type { AgentEvent as PiEvent, SnapshotEvent } from "@earendil-works/pi-durable";
import type { Message } from "@earendil-works/pi-ai";
import type { AgentActivity, AgentPhase, AgentUpdate, RequestContext } from "@buckets/plugin-sdk";

const MAX_TEXT = 65536;
const MAX_MODEL_JSON = 8 * 1024 * 1024;
const CADENCE_MS = 40;

export function assertAnswerText(answer: string): void {
  if (Buffer.byteLength(answer) > MAX_TEXT)
    throw new Error("The answer exceeds the 64 KiB display limit");
  for (const char of answer) {
    const code = char.charCodeAt(0);
    if (char.length === 1 && code >= 0xd800 && code <= 0xdfff)
      throw new Error("The model returned invalid Unicode");
  }
}

/** Decode a possibly unfinished JSON string without exposing incomplete UTF-16
 * pairs or escape syntax. Keys/other values must be complete before scanning on. */
function jsonString(
  source: string,
  start: number,
): { text: string; end: number; complete: boolean } {
  let output = "";
  for (let i = start + 1; i < source.length;) {
    const code = source.charCodeAt(i);
    if (source[i] === '"') return { text: output, end: i + 1, complete: true };
    if (code < 32) throw new Error("The model returned an invalid answer format");
    if (source[i] === "\\") {
      if (i + 1 >= source.length) break;
      const escape = source[i + 1];
      const escaped: Record<string, string> = {
        '"': '"',
        "\\": "\\",
        "/": "/",
        b: "\b",
        f: "\f",
        n: "\n",
        r: "\r",
        t: "\t",
      };
      if (escape !== "u") {
        if (!(escape in escaped)) throw new Error("The model returned an invalid answer format");
        output += escaped[escape];
        i += 2;
        continue;
      }
      if (i + 6 > source.length) break;
      if (!/^[0-9a-f]{4}$/i.test(source.slice(i + 2, i + 6)))
        throw new Error("The model returned an invalid answer format");
      const value = parseInt(source.slice(i + 2, i + 6), 16);
      if (value >= 0xd800 && value <= 0xdbff) {
        if (i + 12 > source.length) break;
        if (
          source.slice(i + 6, i + 8) !== "\\u" ||
          !/^[0-9a-f]{4}$/i.test(source.slice(i + 8, i + 12))
        )
          throw new Error("The model returned invalid Unicode");
        const low = parseInt(source.slice(i + 8, i + 12), 16);
        if (low < 0xdc00 || low > 0xdfff) throw new Error("The model returned invalid Unicode");
        output += String.fromCharCode(value, low);
        i += 12;
        continue;
      }
      if (value >= 0xdc00 && value <= 0xdfff) throw new Error("The model returned invalid Unicode");
      output += String.fromCharCode(value);
      i += 6;
      continue;
    }
    if (code >= 0xd800 && code <= 0xdbff) {
      if (i + 1 >= source.length) break;
      const low = source.charCodeAt(i + 1);
      if (low < 0xdc00 || low > 0xdfff) throw new Error("The model returned invalid Unicode");
      output += source.slice(i, i + 2);
      i += 2;
      continue;
    }
    if (code >= 0xdc00 && code <= 0xdfff) throw new Error("The model returned invalid Unicode");
    output += source[i++];
  }
  return { text: output, end: source.length, complete: false };
}

/** Find only the top-level answer string, regardless of field order. Previewing
 * a JSON envelope is not parsing/accepting its citations or result metadata. */
export function answerPrefix(raw: string): string {
  if (Buffer.byteLength(raw) > MAX_MODEL_JSON)
    throw new Error("The model response exceeds the supported size");
  let source = raw.trimStart();
  if (source.startsWith("```")) {
    const newline = source.indexOf("\n");
    if (newline < 0) return "";
    source = source.slice(newline + 1).trimStart();
  }
  if (!source.startsWith("{")) return "";
  let i = 1;
  const whitespace = () => {
    while (/\s/.test(source[i] ?? "") && i < source.length) i++;
  };
  while (i < source.length) {
    whitespace();
    if (source[i] === "}") return "";
    if (source[i] !== '"') return "";
    const key = jsonString(source, i);
    if (!key.complete) return "";
    i = key.end;
    whitespace();
    if (source[i++] !== ":") return "";
    whitespace();
    if (key.text === "answer") {
      if (source[i] !== '"') return "";
      const answer = jsonString(source, i).text;
      assertAnswerText(answer);
      return answer;
    }
    // Skip another top-level value, including quoted/nested occurrences of answer.
    const stack: string[] = [];
    for (; i < source.length; i++) {
      const char = source[i];
      if (char === '"') {
        const string = jsonString(source, i);
        if (!string.complete) return "";
        i = string.end - 1;
      } else if (char === "{" || char === "[") stack.push(char);
      else if (char === "}" || char === "]") {
        if (!stack.length) break;
        if (stack.pop() !== (char === "}" ? "{" : "[")) return "";
      } else if (char === "," && !stack.length) break;
    }
    if (source[i] !== ",") return "";
    i++;
  }
  return "";
}

function boundedTitle(value: string, max = 512): string {
  const cleaned = Buffer.from(value.replace(/\p{Cc}/gu, " ")).toString("utf8");
  let result = "";
  let bytes = 0;
  for (const char of cleaned) {
    const size = Buffer.byteLength(char);
    if (bytes + size > max) break;
    result += char;
    bytes += size;
  }
  return result;
}
function toolTitle(name: string, args?: Record<string, unknown>): string {
  const label: Record<string, string> = {
    workspace_snapshot: "Inspect open panes",
    files_read: "Read file",
    files_list: "List directory",
    files_search: "Search files",
  };
  const location = typeof args?.location === "string" ? ` · ${args.location}` : "";
  return boundedTitle((Object.hasOwn(label, name) ? label[name] : "Use workspace tool") + location);
}

export class StreamingProgress {
  private requestId: string;
  private rpc: RequestContext;
  private abort: () => void;
  private blocks = new Map<number, string>();
  private activities = new Map<string, AgentActivity>();
  private sequence = 0;
  private phase: AgentPhase = "waiting";
  private timer?: ReturnType<typeof setTimeout>;
  private lastPublished = 0;
  private closed = false;
  private previous = "";
  failure?: Error;
  constructor(requestId: string, rpc: RequestContext, abort: () => void) {
    this.requestId = requestId;
    this.rpc = rpc;
    this.abort = abort;
  }
  start(snapshot: SnapshotEvent): void {
    this.snapshot(snapshot);
    this.publish(true);
  }
  private message(message: Message): void {
    if (message.role !== "assistant") return;
    this.blocks.clear();
    message.content.forEach((block, index) => {
      if (block.type === "text") this.blocks.set(index, block.text);
    });
  }
  private snapshot(snapshot: SnapshotEvent): void {
    this.blocks.clear();
    if (snapshot.generation?.message) this.message(snapshot.generation.message);
    const results = new Map<string, boolean>();
    for (const entry of snapshot.entries) {
      for (const message of entry.model ?? [])
        if (message.role === "toolResult") results.set(message.toolCallId, message.isError);
    }
    const slots = [...snapshot.tools, ...snapshot.nestedTools];
    const present = new Set(slots.map((slot) => slot.callId));
    for (const [id, previous] of this.activities) {
      if (previous.state === "running" && !present.has(id)) {
        previous.state = results.has(id) && !results.get(id) ? "complete" : "error";
      }
    }
    for (const slot of slots) {
      const isError =
        "summary" in slot
          ? (slot.summary?.isError ?? true)
          : (results.get(slot.callId) ?? !("entry" in slot && slot.entry !== undefined));
      this.activity(
        slot.callId,
        slot.name,
        slot.status === "running" || slot.status === "pending"
          ? "running"
          : isError
            ? "error"
            : "complete",
      );
    }
    this.phase = [...this.activities.values()].some((activity) => activity.state === "running")
      ? "reading"
      : this.blocks.size
        ? "responding"
        : "waiting";
  }
  private activity(
    id: string,
    name: string,
    state: AgentActivity["state"],
    args?: Record<string, unknown>,
  ): void {
    const old = this.activities.get(id);
    this.activities.set(id, {
      id: createHash("sha256").update(id).digest("hex"),
      title: args || !old ? toolTitle(name, args) : old.title,
      state,
    });
    while (this.activities.size > 32) this.activities.delete(this.activities.keys().next().value!);
  }
  accept(events: readonly PiEvent[]): void {
    if (this.closed || this.rpc.signal.aborted || this.failure || this.phase === "verifying")
      return;
    let urgent = false;
    for (const event of events) {
      switch (event.type) {
        case "snapshot":
          this.snapshot(event);
          urgent = true;
          break;
        case "message_start":
          this.message(event.message);
          break;
        case "message_update":
          for (const change of event.changes) {
            if (change.type === "message") this.message(change.message);
            else if (change.type === "text_delta")
              this.blocks.set(
                change.contentIndex,
                (this.blocks.get(change.contentIndex) ?? "") + change.delta,
              );
            else if (
              (change.type === "text_start" || change.type === "block") &&
              change.block.type === "text"
            )
              this.blocks.set(change.contentIndex, change.block.text);
          }
          if (this.blocks.size) this.phase = "responding";
          break;
        case "message_end":
          for (const message of event.entry.model ?? []) this.message(message);
          break;
        case "tool_execution_start":
          this.activity(event.toolCallId, event.toolName, "running", event.args);
          this.phase = "reading";
          urgent = true;
          break;
        case "tool_execution_end": {
          const error =
            event.entry?.model?.some(
              (message) => message.role === "toolResult" && message.isError,
            ) ||
            event.result?.isError ||
            (!event.entry && !event.result);
          this.activity(event.toolCallId, event.toolName, error ? "error" : "complete");
          this.phase = [...this.activities.values()].some(
            (activity) => activity.state === "running",
          )
            ? "reading"
            : "waiting";
          urgent = true;
          break;
        }
        // Thinking, raw retry errors, tool output, and task diagnostics stay private.
      }
    }
    this.publish(urgent);
  }
  verifying(raw: string): void {
    this.blocks.clear();
    this.blocks.set(0, raw);
    this.phase = "verifying";
    this.publish(true);
  }
  private publish(force = false): void {
    if (this.closed || this.rpc.signal.aborted || this.failure) return;
    const wait = CADENCE_MS - (Date.now() - this.lastPublished);
    if (!force && wait > 0) {
      this.timer ??= setTimeout(() => {
        this.timer = undefined;
        this.publish();
      }, wait);
      return;
    }
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = undefined;
    }
    try {
      const raw = [...this.blocks]
        .sort(([a], [b]) => a - b)
        .map(([, text]) => text)
        .join("");
      const update: AgentUpdate = {
        sequence: this.sequence + 1,
        text: answerPrefix(raw),
        phase: this.phase,
        activities: [...this.activities.values()],
      };
      const serialized = JSON.stringify({ ...update, sequence: 0 });
      if (serialized === this.previous) return;
      this.previous = serialized;
      this.lastPublished = Date.now();
      this.sequence++;
      const labels: Record<AgentPhase, string> = {
        waiting: "Thinking",
        reading: "Reading workspace",
        responding: "Responding",
        verifying: "Verifying cited files",
      };
      this.rpc.notify("agent.progress", {
        request_id: this.requestId,
        message: labels[this.phase],
        update,
      });
    } catch (error) {
      this.failure = error instanceof Error ? error : new Error("Could not display the response");
      this.abort();
    }
  }
  close(): void {
    this.closed = true;
    if (this.timer) clearTimeout(this.timer);
    this.timer = undefined;
  }
}
