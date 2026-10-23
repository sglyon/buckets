/** ChatGPT authorization belongs to the plugin, protected persistence to the host.
 * This module never returns credentials through the public account protocol. */
import {
  createHash,
  createPublicKey,
  randomBytes,
  randomUUID,
  timingSafeEqual,
  verify,
} from "node:crypto";
import { createServer, type Server } from "node:http";
import { setTimeout as delay } from "node:timers/promises";
import type {
  AccountModel,
  AccountRequest,
  AccountState,
  RequestContext,
} from "@buckets/plugin-sdk";

const AUTH = "https://auth.openai.com";
const API = "https://api.openai.com/v1";
const DIRECT_SCOPE = "chatgpt.tokens.use.direct";
const SCOPES = `openid profile email offline_access resource.invoke ${DIRECT_SCOPE}`;
export const CHATGPT_USAGE_URL = "https://chatgpt.com/settings/usage";
const TERMINAL_REFRESH = new Set([
  "invalid_grant",
  "invalid_refresh_token",
  "token_expired",
  "refresh_token_expired",
  "refresh_token_invalidated",
  "refresh_token_reused",
]);
interface Tokens {
  access: string;
  refresh: string;
  id_token: string;
  expires_at: number;
  scopes: string[];
  earliest_refresh_at?: number;
}
interface Account {
  id: string;
  client_id: string;
  subject?: string;
  issuer?: string;
  email?: string;
  tokens?: Tokens;
  models: AccountModel[];
  welcomed?: boolean;
  session_version?: number;
}
interface RecordData {
  version: 1;
  host_id: string;
  active_account_id?: string;
  accounts: Account[];
}
interface Discovery {
  issuer: string;
  jwks_uri: string;
  revocation_endpoint: string;
}
/** Injection is code-only for loopback fixtures, never exposed in plugin settings. */
export interface AccountOptions {
  fetch?: typeof fetch;
  issuer?: string;
  api?: string;
  now?: () => number;
  signInTimeoutMs?: number;
}
export interface ModelConnection {
  accountId: string;
  model: AccountModel;
  token: string;
}
class SafeAuthError extends Error {}
class TokenError extends SafeAuthError {
  terminal: boolean;
  constructor(terminal: boolean) {
    super(
      terminal
        ? "ChatGPT session expired. Continue with ChatGPT to reconnect."
        : "ChatGPT could not renew the session. Check your connection and try again.",
    );
    this.terminal = terminal;
  }
}
function safeEqual(a: string, b: string): boolean {
  const left = Buffer.from(a);
  const right = Buffer.from(b);
  return left.length === right.length && timingSafeEqual(left, right);
}
function text(value: unknown, max = 32768): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= max &&
    !/[\x00-\x1f]/.test(value)
  );
}
function record(value: unknown): RecordData {
  if (value === undefined || value === null)
    return { version: 1, host_id: `urn:uuid:${randomUUID()}`, accounts: [] };
  const data = value as RecordData;
  if (
    data.version !== 1 ||
    !/^urn:uuid:[0-9a-f-]{36}$/i.test(data.host_id) ||
    !Array.isArray(data.accounts) ||
    data.accounts.length > 64 ||
    new Set(data.accounts.map((account) => account.id)).size !== data.accounts.length
  )
    throw new SafeAuthError(
      "ChatGPT account storage is invalid. Restore the protected account file before continuing.",
    );
  for (const account of data.accounts) {
    if (
      !text(account.id, 128) ||
      !text(account.client_id, 256) ||
      account.client_id === "dynamic_agent_client" ||
      !Array.isArray(account.models) ||
      account.models.length > 512
    )
      throw new SafeAuthError("ChatGPT account storage is invalid.");
    if (
      account.tokens &&
      (!text(account.tokens.access) ||
        !text(account.tokens.refresh) ||
        !text(account.tokens.id_token) ||
        !Number.isFinite(account.tokens.expires_at) ||
        !Array.isArray(account.tokens.scopes))
    )
      throw new SafeAuthError("ChatGPT account storage is invalid.");
  }
  return structuredClone(data);
}
function active(data: RecordData, id = data.active_account_id): Account | undefined {
  return data.accounts.find((account) => account.id === id);
}
function plan(account: Account | undefined): boolean {
  return Boolean(
    account?.tokens?.scopes.includes(DIRECT_SCOPE) &&
    account.tokens.scopes.includes("resource.invoke"),
  );
}
function publicState(
  data: RecordData,
  provider: string,
  message?: string,
  firstConnection = false,
): AccountState {
  const selected = active(data);
  const enabled = plan(selected);
  return {
    accounts: data.accounts.map((account) => ({
      id: account.id,
      label: `${account.email ?? "ChatGPT account"} · ${account.client_id.slice(-8)}`,
      connected: Boolean(account.tokens),
      plan_enabled: plan(account),
    })),
    active_account_id: data.active_account_id,
    models: selected?.tokens ? selected.models : [],
    message,
    plan_enabled: enabled,
    first_connection: firstConnection,
    using_plan: provider === "chatgpt" && enabled,
    sign_in_required: provider === "chatgpt" && !enabled,
    sign_in_label: "Continue with ChatGPT",
    plan_label: "Using ChatGPT plan",
    usage_url: CHATGPT_USAGE_URL,
  };
}

export class ChatGPTAccounts {
  private rpc: RequestContext;
  private network: typeof fetch;
  private issuer: string;
  private api: string;
  private now: () => number;
  private options: AccountOptions;
  constructor(rpc: RequestContext, options: AccountOptions = {}) {
    this.rpc = rpc;
    this.options = options;
    this.network = options.fetch ?? fetch;
    this.issuer = options.issuer ?? AUTH;
    this.api = options.api ?? API;
    this.now = options.now ?? Date.now;
  }
  private async read(): Promise<RecordData> {
    const value = await this.rpc.host.call<{ value?: unknown }>("credentials.read", {
      key: "chatgpt",
    });
    return record(value.value);
  }
  private async modify<T>(change: (data: RecordData) => Promise<T>): Promise<T> {
    let acquired: { lease: string; value?: unknown } | undefined;
    for (let retry = 0; !acquired; retry++) {
      this.rpc.signal.throwIfAborted();
      try {
        acquired = await this.rpc.host.call("credentials.acquire", {
          key: "chatgpt",
        });
      } catch (error) {
        if (
          retry >= 600 ||
          !/busy|locked|already.*lease/i.test(error instanceof Error ? error.message : "")
        )
          throw new SafeAuthError(
            "ChatGPT account storage is unavailable. Try again after the other account operation finishes.",
          );
        await delay(100, undefined, { signal: this.rpc.signal });
      }
    }
    let committed = false;
    try {
      const data = record(acquired.value);
      const result = await change(data);
      this.rpc.signal.throwIfAborted();
      await this.rpc.host.call("credentials.commit", {
        lease: acquired.lease,
        value: data,
      });
      committed = true;
      return result;
    } finally {
      if (!committed)
        await this.rpc.host.call("credentials.release", { lease: acquired.lease }).catch(() => {});
    }
  }
  private async json(url: string, init: RequestInit = {}): Promise<unknown> {
    const response = await this.network(url, {
      ...init,
      redirect: "error",
      signal: AbortSignal.any([this.rpc.signal, AbortSignal.timeout(15000)]),
    });
    if (!response.ok)
      throw new SafeAuthError(
        "ChatGPT is temporarily unavailable. Check your connection and try again.",
      );
    const raw = await response.text();
    if (raw.length > 1048576) throw new SafeAuthError("ChatGPT returned an invalid response.");
    return JSON.parse(raw);
  }
  private async discovery(): Promise<Discovery> {
    const value = (await this.json(`${this.issuer}/.well-known/openid-configuration`)) as Discovery;
    if (
      value.issuer !== this.issuer ||
      !text(value.jwks_uri, 8192) ||
      !text(value.revocation_endpoint, 8192)
    )
      throw new SafeAuthError("ChatGPT identity metadata could not be verified.");
    for (const candidate of [value.jwks_uri, value.revocation_endpoint]) {
      const url = new URL(candidate);
      if (
        url.origin !== new URL(this.issuer).origin ||
        url.username ||
        url.password ||
        url.hash ||
        url.search
      )
        throw new SafeAuthError("ChatGPT identity metadata could not be verified.");
    }
    return value;
  }
  private async identity(
    idToken: string,
    clientId: string,
    nonce: string,
  ): Promise<{ subject: string; email?: string; issuer: string }> {
    const parts = idToken.split(".");
    if (parts.length !== 3 || parts.some((part) => !/^[A-Za-z0-9_-]+$/.test(part)))
      throw new SafeAuthError("ChatGPT identity could not be verified.");
    const header = JSON.parse(Buffer.from(parts[0], "base64url").toString()) as {
      alg?: string;
      kid?: string;
      crit?: unknown;
    };
    const claims = JSON.parse(Buffer.from(parts[1], "base64url").toString()) as {
      iss?: string;
      aud?: string | string[];
      azp?: string;
      exp?: number;
      iat?: number;
      nbf?: number;
      nonce?: string;
      sub?: string;
      email?: string;
      email_verified?: boolean;
    };
    if (
      !["RS256", "ES256"].includes(header.alg ?? "") ||
      !text(header.kid, 256) ||
      header.crit !== undefined
    )
      throw new SafeAuthError("ChatGPT identity could not be verified.");
    const metadata = await this.discovery();
    const jwks = (await this.json(metadata.jwks_uri)) as {
      keys?: JsonWebKey[];
    };
    if (!Array.isArray(jwks.keys) || jwks.keys.length > 64)
      throw new SafeAuthError("ChatGPT identity keys are unavailable.");
    const matching = jwks.keys.filter(
      (key) =>
        (key as JsonWebKey & { kid?: string }).kid === header.kid &&
        (!key.alg || key.alg === header.alg) &&
        (!key.use || key.use === "sig") &&
        (!key.key_ops || key.key_ops.includes("verify")),
    );
    if (matching.length !== 1 || matching[0].kty !== (header.alg === "RS256" ? "RSA" : "EC"))
      throw new SafeAuthError("ChatGPT identity could not be verified.");
    const key = createPublicKey({
      key: matching[0] as import("node:crypto").JsonWebKey,
      format: "jwk",
    });
    const valid = verify(
      "sha256",
      Buffer.from(`${parts[0]}.${parts[1]}`),
      {
        key,
        ...(header.alg === "ES256" ? { dsaEncoding: "ieee-p1363" as const } : {}),
      },
      Buffer.from(parts[2], "base64url"),
    );
    const audiences = typeof claims.aud === "string" ? [claims.aud] : claims.aud;
    if (
      !valid ||
      claims.iss !== this.issuer ||
      !Array.isArray(audiences) ||
      !audiences.includes(clientId) ||
      (audiences.length > 1 && claims.azp !== clientId) ||
      (claims.azp !== undefined && claims.azp !== clientId) ||
      typeof claims.exp !== "number" ||
      claims.exp * 1000 <= this.now() ||
      (claims.nbf !== undefined && claims.nbf * 1000 > this.now() + 30000) ||
      (claims.iat !== undefined && claims.iat * 1000 > this.now() + 30000) ||
      !text(claims.nonce, 512) ||
      !safeEqual(claims.nonce, nonce) ||
      !text(claims.sub, 512)
    )
      throw new SafeAuthError("ChatGPT identity could not be verified.");
    return {
      subject: claims.sub,
      issuer: claims.iss,
      ...(text(claims.email, 320) ? { email: claims.email } : {}),
    };
  }
  private async token(body: URLSearchParams, previous?: Tokens): Promise<Tokens> {
    const response = await this.network(`${this.issuer}/api/accounts/oauth/token`, {
      method: "POST",
      headers: {
        "content-type": "application/x-www-form-urlencoded",
        accept: "application/json",
      },
      body,
      redirect: "error",
      signal: AbortSignal.any([this.rpc.signal, AbortSignal.timeout(15000)]),
    });
    const raw = await response.text();
    if (raw.length > 262144) throw new TokenError(false);
    let value: Record<string, unknown>;
    try {
      value = JSON.parse(raw);
    } catch {
      throw new TokenError(false);
    }
    if (!response.ok)
      throw new TokenError(typeof value.error === "string" && TERMINAL_REFRESH.has(value.error));
    const scopes =
      value.scope === undefined && previous
        ? previous.scopes
        : text(value.scope, 4096)
          ? value.scope.split(/\s+/).filter(Boolean)
          : undefined;
    // SIWC rotates the renewable credential on every successful exchange.
    // An omitted replacement is malformed, never permission to reuse a token
    // the server may already have consumed. The enclosing lease preserves the
    // prior record on this failure, allowing an explicit retry/reconnection.
    const refresh = value.refresh_token;
    const idToken = value.id_token ?? previous?.id_token;
    if (
      !text(value.access_token) ||
      !text(refresh) ||
      !text(idToken) ||
      !scopes ||
      typeof value.expires_in !== "number" ||
      !Number.isFinite(value.expires_in) ||
      value.expires_in <= 0 ||
      (value.token_type !== undefined && String(value.token_type).toLowerCase() !== "bearer")
    )
      throw new TokenError(false);
    return {
      access: value.access_token,
      refresh,
      id_token: idToken,
      expires_at: this.now() + value.expires_in * 1000,
      scopes,
      ...(typeof value.earliest_refresh_at === "number"
        ? { earliest_refresh_at: value.earliest_refresh_at * 1000 }
        : {}),
    };
  }
  private async credential(id?: string): Promise<{ account: Account; token: string }> {
    let expired = false;
    const result = await this.modify(async (data) => {
      const account = active(data, id);
      if (!account?.tokens || !plan(account))
        throw new SafeAuthError(
          "Continue with ChatGPT in Settings → Plugins to use your ChatGPT plan.",
        );
      if (id && data.active_account_id !== id)
        throw new SafeAuthError(
          "The ChatGPT account changed. Submit a new question for the selected account.",
        );
      if (
        account.tokens.expires_at <= this.now() + 60000 &&
        (!account.tokens.earliest_refresh_at || account.tokens.earliest_refresh_at <= this.now())
      ) {
        try {
          account.tokens = await this.token(
            new URLSearchParams({
              grant_type: "refresh_token",
              client_id: account.client_id,
              refresh_token: account.tokens.refresh,
              resource: API,
            }),
            account.tokens,
          );
        } catch (error) {
          if (error instanceof TokenError && error.terminal) {
            delete account.tokens;
            account.models = [];
            expired = true;
            return undefined;
          }
          throw error;
        }
      }
      if (account.tokens.expires_at <= this.now())
        throw new SafeAuthError(
          "ChatGPT session is awaiting renewal. Try again shortly or reconnect.",
        );
      if (!plan(account))
        throw new SafeAuthError(
          "ChatGPT plan usage is not enabled. Continue with ChatGPT to enable it.",
        );
      return {
        account: structuredClone(account),
        token: account.tokens.access,
      };
    });
    if (expired || !result)
      throw new SafeAuthError("ChatGPT session expired. Continue with ChatGPT to reconnect.");
    return result;
  }
  async assertActive(id: string): Promise<void> {
    const data = await this.read();
    if (data.active_account_id !== id || !plan(active(data, id)))
      throw new SafeAuthError(
        "The ChatGPT account changed or disconnected. Submit a new question after connecting.",
      );
  }
  async accessToken(id: string): Promise<string> {
    return (await this.credential(id)).token;
  }
  async modelConnection(modelId: string): Promise<ModelConnection> {
    const resolved = await this.credential();
    let models = resolved.account.models;
    if (!models.length) models = await this.loadModels(resolved.account.id);
    const model = modelId ? models.find((model) => model.id === modelId) : models[0];
    if (!model) throw new SafeAuthError("Choose an available ChatGPT model in Settings → Plugins.");
    await this.assertActive(resolved.account.id);
    return { accountId: resolved.account.id, model, token: resolved.token };
  }
  private async loadModels(id?: string): Promise<AccountModel[]> {
    const resolved = await this.credential(id);
    const value = (await this.json(`${this.api}/models`, {
      headers: { Authorization: `Bearer ${resolved.token}` },
    })) as {
      models?: {
        slug?: unknown;
        display_name?: unknown;
        visibility?: unknown;
      }[];
    };
    if (!Array.isArray(value.models) || value.models.length > 2048)
      throw new SafeAuthError("ChatGPT returned an invalid model list.");
    const models: AccountModel[] = [];
    for (const item of value.models)
      if (
        item.visibility === "list" &&
        text(item.slug, 256) &&
        text(item.display_name, 256) &&
        !models.some((model) => model.id === item.slug)
      )
        models.push({ id: item.slug, name: item.display_name });
    if (!models.length)
      throw new SafeAuthError(
        "No ChatGPT models are available for this account. Manage usage or choose another account.",
      );
    await this.modify(async (data) => {
      const current = active(data, resolved.account.id);
      if (
        !current?.tokens ||
        data.active_account_id !== resolved.account.id ||
        current.subject !== resolved.account.subject ||
        current.client_id !== resolved.account.client_id
      )
        throw new SafeAuthError("The ChatGPT account changed. Refresh models again.");
      current.models = models;
    });
    return models;
  }
  async invoke(request: AccountRequest, provider = "chatgpt"): Promise<AccountState> {
    try {
      if (
        !request ||
        !["status", "sign_in", "sign_out", "select", "models"].includes(request.action) ||
        (request.account_id !== undefined && !text(request.account_id, 128))
      )
        throw new SafeAuthError("Invalid account action.");
      if (request.action === "status") {
        const data = await this.read();
        if (provider === "chatgpt" && plan(active(data)) && !active(data)?.welcomed) {
          const current = await this.modify(async (data) => {
            const account = active(data);
            const first = Boolean(account && plan(account) && !account.welcomed);
            if (account && first) account.welcomed = true;
            return { data, first };
          });
          return publicState(
            current.data,
            provider,
            current.first
              ? "You’re using your ChatGPT plan. Eligible AI requests in Buckets use your plan; manage usage in ChatGPT settings."
              : undefined,
            current.first,
          );
        }
        return publicState(data, provider);
      }
      if (request.action === "sign_in") return await this.signIn(request.account_id, provider);
      if (request.action === "select") {
        const data = await this.modify(async (data) => {
          const account = active(data, request.account_id);
          if (!request.account_id || !account?.tokens)
            throw new SafeAuthError("Connect this ChatGPT account before selecting it.");
          data.active_account_id = account.id;
          return data;
        });
        return publicState(data, provider);
      }
      if (request.action === "models") {
        await this.loadModels(request.account_id);
        return publicState(await this.read(), provider);
      }
      return await this.signOut(request.account_id, provider);
    } catch (error) {
      if (this.rpc.signal.aborted) throw new SafeAuthError("ChatGPT account operation cancelled.");
      if (error instanceof SafeAuthError) throw error;
      throw new SafeAuthError("ChatGPT connection failed. Check your connection and try again.");
    }
  }
  private async signIn(id: string | undefined, provider: string): Promise<AccountState> {
    const snapshot = await this.modify(async (data) => data); // Persist host ID before opening the browser.
    const selected = id ? active(snapshot, id) : undefined;
    if (id && !selected) throw new SafeAuthError("The selected ChatGPT account is unavailable.");
    const signal = AbortSignal.any([
      this.rpc.signal,
      AbortSignal.timeout(this.options.signInTimeoutMs ?? 180000),
    ]);
    const state = randomBytes(32).toString("base64url");
    const nonce = randomBytes(32).toString("base64url");
    const verifier = randomBytes(48).toString("base64url");
    const challenge = createHash("sha256").update(verifier).digest("base64url");
    let server: Server | undefined;
    let rejectCallback: (reason?: unknown) => void = () => {};
    let redirect = "";
    const callback = new Promise<{ code: string; clientId: string }>((resolve, reject) => {
      rejectCallback = reject;
      server = createServer((req, res) => {
        const reply = (status: number, body: string) => {
          res.writeHead(status, {
            "Content-Type": "text/plain; charset=utf-8",
            "Cache-Control": "no-store",
            "Content-Security-Policy": "default-src 'none'",
            "Referrer-Policy": "no-referrer",
          });
          res.end(body);
        };
        try {
          const url = new URL(req.url ?? "", redirect);
          if (req.method !== "GET" || url.pathname !== "/auth/callback") {
            reply(404, "Not found");
            return;
          }
          if (
            req.headers.host !== new URL(redirect).host ||
            !safeEqual(url.searchParams.get("state") ?? "", state)
          ) {
            reply(400, "Invalid sign-in callback");
            return;
          }
          if (url.searchParams.has("error")) {
            reply(400, "ChatGPT was not connected. Return to Buckets.");
            reject(
              new SafeAuthError(
                "ChatGPT sign-in was declined. You can continue with an API key or local provider.",
              ),
            );
            return;
          }
          const code = url.searchParams.get("code");
          const issued = url.searchParams.get("client_id");
          const clientId = issued ?? selected?.client_id;
          if (
            !text(code, 8192) ||
            !text(clientId, 256) ||
            clientId === "dynamic_agent_client" ||
            (selected && issued && issued !== selected.client_id)
          ) {
            reply(400, "Invalid sign-in callback");
            reject(new SafeAuthError("ChatGPT registration did not complete. Try again."));
            return;
          }
          reply(200, "Authorization received. Return to Buckets to finish connecting.");
          resolve({ code, clientId });
        } catch {
          reply(400, "Invalid sign-in callback");
        }
      });
      server.on("error", () =>
        reject(new SafeAuthError("The ChatGPT sign-in callback could not start. Try again.")),
      );
    });
    // Attach rejection before opening the listener to avoid an unhandled abort.
    void callback.catch(() => {});
    const abort = () =>
      rejectCallback(new SafeAuthError("ChatGPT sign-in cancelled or timed out. Try again."));
    signal.addEventListener("abort", abort, { once: true });
    try {
      await new Promise<void>((resolve, reject) => {
        server!.once("error", reject);
        server!.listen(0, "127.0.0.1", () => {
          server!.removeListener("error", reject);
          resolve();
        });
      });
      const address = server!.address();
      if (!address || typeof address === "string")
        throw new SafeAuthError("The ChatGPT callback could not start.");
      redirect = `http://127.0.0.1:${address.port}/auth/callback`;
      const authorization = new URL(`${this.issuer}/api/accounts/authorize`);
      authorization.search = new URLSearchParams({
        client_id: selected?.client_id ?? "dynamic_agent_client",
        ext_agent_host_id: snapshot.host_id,
        response_type: "code",
        redirect_uri: redirect,
        resource: API,
        scope: SCOPES,
        state,
        nonce,
        code_challenge: challenge,
        code_challenge_method: "S256",
        ...(!selected ? { agent_name_hint: "Buckets" } : {}),
        ...(selected?.tokens?.id_token ? { id_token_hint: selected.tokens.id_token } : {}),
        ...(selected?.email ? { login_hint: selected.email } : {}),
        ...(selected?.tokens && !plan(selected) ? { prompt: "consent" } : {}),
      }).toString();
      signal.throwIfAborted();
      this.rpc.notify("accounts.progress", {
        message: "Complete ChatGPT sign-in in your browser…",
      });
      await this.rpc.host.call("browser.open", {
        url: authorization.toString(),
      });
      const result = await callback;
      signal.throwIfAborted();
      const registration = await this.modify(async (data) => {
        if (selected) {
          const current = active(data, selected.id);
          if (
            !current ||
            current.client_id !== result.clientId ||
            (current.session_version ?? 0) !== (selected.session_version ?? 0)
          )
            throw new SafeAuthError("The ChatGPT account registration changed. Try again.");
          return { id: current.id, version: current.session_version ?? 0 };
        }
        if (data.accounts.length >= 64) throw new SafeAuthError("Too many saved ChatGPT accounts.");
        const account: Account = {
          id: randomUUID(),
          client_id: result.clientId,
          models: [],
        };
        data.accounts.push(account);
        return { id: account.id, version: 0 };
      });
      this.rpc.notify("accounts.progress", {
        message: "Verifying ChatGPT connection…",
      });
      const tokens = await this.token(
        new URLSearchParams({
          grant_type: "authorization_code",
          client_id: result.clientId,
          code: result.code,
          code_verifier: verifier,
          redirect_uri: redirect,
          resource: API,
        }),
      );
      signal.throwIfAborted();
      const identity = await this.identity(tokens.id_token, result.clientId, nonce);
      signal.throwIfAborted();
      const connected = await this.modify(async (data) => {
        const current = active(data, registration.id);
        if (
          !current ||
          current.client_id !== result.clientId ||
          (current.session_version ?? 0) !== registration.version ||
          (current.subject &&
            (current.subject !== identity.subject || current.issuer !== identity.issuer))
        )
          throw new SafeAuthError(
            "ChatGPT returned a different or changed account. Add it as a separate connection.",
          );
        const first =
          provider === "chatgpt" &&
          !current.welcomed &&
          tokens.scopes.includes(DIRECT_SCOPE) &&
          tokens.scopes.includes("resource.invoke");
        Object.assign(current, identity, {
          tokens,
          models: [],
          session_version: registration.version + 1,
        });
        if (first) current.welcomed = true;
        data.active_account_id = current.id;
        return { first, enabled: plan(current) };
      });
      let message = connected.enabled
        ? provider === "chatgpt"
          ? "ChatGPT plan connected."
          : "ChatGPT plan connected. Your selected API or local model provider is unchanged."
        : "Signed in. ChatGPT plan usage is not enabled. Continue with ChatGPT to grant permission.";
      if (connected.enabled)
        try {
          await this.loadModels(registration.id);
        } catch {
          message = "ChatGPT plan connected. Refresh models when your connection is available.";
        }
      if (connected.first)
        message =
          "You’re using your ChatGPT plan. Eligible AI requests in Buckets use your plan; manage usage in ChatGPT settings.";
      return publicState(await this.read(), provider, message, connected.first);
    } finally {
      signal.removeEventListener("abort", abort);
      server?.closeAllConnections();
      await new Promise<void>((resolve) => {
        if (!server?.listening) resolve();
        else server.close(() => resolve());
      });
    }
  }
  private async signOut(id: string | undefined, provider: string): Promise<AccountState> {
    let revoked = false;
    const data = await this.modify(async (data) => {
      const account = active(data, id);
      if (!account) throw new SafeAuthError("The selected ChatGPT account is unavailable.");
      if (account.tokens) {
        try {
          const discovery = await this.discovery();
          for (let attempt = 0; attempt < 2; attempt++) {
            const response = await this.network(discovery.revocation_endpoint, {
              method: "POST",
              headers: { "content-type": "application/x-www-form-urlencoded" },
              body: new URLSearchParams({
                token: account.tokens.refresh,
                token_type_hint: "refresh_token",
                client_id: account.client_id,
              }),
              redirect: "error",
              signal: AbortSignal.any([this.rpc.signal, AbortSignal.timeout(10000)]),
            });
            if (response.status === 200) {
              revoked = true;
              break;
            }
            if (response.status < 500) break;
            if (attempt === 0) await delay(200, undefined, { signal: this.rpc.signal });
          }
        } catch {
          /* Local disconnect still succeeds; report unconfirmed remote revocation. */
        }
      } else revoked = true;
      delete account.tokens;
      account.models = [];
      account.session_version = (account.session_version ?? 0) + 1;
      if (data.active_account_id === account.id) delete data.active_account_id;
      return data;
    });
    return publicState(
      data,
      provider,
      revoked
        ? "Disconnected from ChatGPT."
        : "Disconnected locally. Remote revocation could not be confirmed; disconnect Buckets in ChatGPT settings.",
    );
  }
}
