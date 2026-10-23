import { test } from "vite-plus/test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { RetrievalIndex, cosine } from "../src/retrieval.ts";
import { readConfig, endpoint } from "../src/config.ts";
import { request, rpc } from "./helpers.ts";

async function fixture(run: (directory: string, url: string, inputs: string[][]) => Promise<void>) {
  const directory = await mkdtemp(join(tmpdir(), "buckets-index-"));
  const inputs: string[][] = [];
  const server = createServer(async (req, res) => {
    const data: Buffer[] = [];
    for await (const chunk of req) data.push(chunk);
    const body = JSON.parse(Buffer.concat(data).toString()) as {
      input: string[];
    };
    inputs.push(body.input);
    const vector = (text: string) =>
      /cat|feline|kitten/i.test(text)
        ? [1, 0, 0]
        : /budget|finance|money/i.test(text)
          ? [0, 1, 0]
          : [0, 0, 1];
    res.setHeader("Content-Type", "application/json");
    res.end(
      JSON.stringify({
        data: body.input.map((text, index) => ({
          index,
          embedding: vector(text),
        })),
      }),
    );
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert.ok(address && typeof address !== "string");
  try {
    await run(directory, `http://127.0.0.1:${address.port}/v1/embeddings`, inputs);
  } finally {
    server.closeAllConnections();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await rm(directory, { recursive: true, force: true });
  }
}
test("real embedding HTTP vectors retrieve a synonym, fuse keywords and reuse/update/remove cached files", async () =>
  fixture(async (directory, url, inputs) => {
    const index = new RetrievalIndex(join(directory, "vectors.sqlite"));
    const files = new Map([
      ["/fixture/animals.txt", "Cats sleep near the window.\nKittens enjoy warm places."],
      ["/fixture/budget.txt", "Money and finance budget for the team."],
    ]);
    const calls: { method: string; params: any }[] = [];
    const context = rpc(async (method, params) => {
      calls.push({ method, params });
      if (method === "files.search") {
        assert.equal(params.query, "");
        assert.equal(params.respect_gitignore, true);
        return {
          results: [...files.keys()].map((location) => ({
            location,
            name: location.split("/").at(-1),
            is_dir: false,
          })),
          truncated: false,
        };
      }
      if (method === "files.read") {
        assert.equal(params.max_bytes, 16384);
        return {
          location: params.location,
          text: files.get(params.location),
          truncated: false,
          binary: false,
        };
      }
      throw new Error("Unexpected tool");
    });
    const config = readConfig({
      embedding_endpoint: url,
      embedding_model: "test-only-embedding",
    });
    const req = request(directory, "search");
    req.prompt = "feline";
    let result = await index.search(req, config, context);
    assert.equal(result.files[0]?.location, "/fixture/animals.txt");
    assert.match(result.files[0]!.reason!, /Embedding similarity 1.000/);
    assert.equal(result.receipt.files, 2);
    assert.equal(inputs.length, 3); // two changed file chunks plus query
    result = await index.search(req, config, context);
    assert.equal(result.receipt.reused, 2);
    assert.equal(inputs.length, 4); // only query embeds again
    files.set("/fixture/animals.txt", "The kitten now lives indoors.");
    result = await index.search(req, config, context);
    assert.equal(result.receipt.reused, 1);
    assert.equal(inputs.length, 6);
    files.delete("/fixture/animals.txt");
    result = await index.search(req, config, context);
    assert.equal(
      result.files.some((file) => file.location === "/fixture/animals.txt"),
      false,
    );
    req.prompt = "budget";
    result = await index.search(req, config, context);
    assert.match(result.files[0]!.reason!, /keyword rank 1/);
    assert.ok(Number.isFinite(result.files[0]!.score));
    assert.ok(
      calls.every((call) => call.method === "files.search" || call.method === "files.read"),
    );
    index.close();
  }));
test("index preserves provider URIs and reports capped/truncated/skipped candidate coverage", async () =>
  fixture(async (directory, url) => {
    const index = new RetrievalIndex(join(directory, "vectors.sqlite"));
    const config = readConfig({
      embedding_endpoint: url,
      embedding_model: "test",
      max_files: 1,
      index_roots: [{ location: "ssh://example.test/work", respect_gitignore: true }],
    });
    const context = rpc(async (method, params) =>
      method === "files.search"
        ? {
            results: [
              { location: "ssh://example.test/work/a", name: "a" },
              { location: "ssh://example.test/work/b", name: "b" },
            ],
            truncated: true,
          }
        : {
            location: params.location,
            text: "cat data",
            binary: false,
            truncated: true,
          },
    );
    const result = await index.search(
      { ...request(directory, "search"), prompt: "feline" },
      config,
      context,
    );
    assert.equal(result.files[0]?.location, "ssh://example.test/work/a");
    assert.equal(result.receipt.truncated, true);
    assert.match(result.receipt.note, /some files truncated/);
    assert.match(result.receipt.note, /traversal capped/);
    index.close();
  }));
test("invalid embedding vectors and missing configuration are explicit errors", async () =>
  fixture(async (directory) => {
    const index = new RetrievalIndex(join(directory, "vectors.sqlite"));
    await assert.rejects(
      index.embed(["hello"], readConfig({}), new AbortController().signal),
      /Configure embedding/,
    );
    const server = createServer((_, response) =>
      response.end(JSON.stringify({ data: [{ index: 0, embedding: [0, 0] }] })),
    );
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    const address = server.address();
    assert.ok(address && typeof address !== "string");
    try {
      await assert.rejects(
        index.embed(
          ["hello"],
          readConfig({
            embedding_endpoint: `http://127.0.0.1:${address.port}`,
            embedding_model: "bad",
          }),
          new AbortController().signal,
        ),
        /invalid vectors/,
      );
    } finally {
      server.closeAllConnections();
      await new Promise<void>((resolve) => server.close(() => resolve()));
      index.close();
    }
  }));
test("extreme finite vectors remain finite and endpoints reject inline credential channels", () => {
  assert.equal(cosine([1e308, 1e308], [1e308, 1e308]), 0.9999999999999998);
  assert.throws(
    () => endpoint("https://example.test/embeddings?api_key=secret", "endpoint"),
    /without credentials/,
  );
  assert.throws(
    () => endpoint("https://name:secret@example.test", "endpoint"),
    /without credentials/,
  );
});
test("directory-heavy roots enumerate files before applying the index cap", async () =>
  fixture(async (directory, url) => {
    const index = new RetrievalIndex(join(directory, "vectors.sqlite"));
    const context = rpc(async (method, params) => {
      if (method === "files.search") {
        // Without files_only, two leading directories consume the bounded page.
        return {
          results: params.files_only
            ? [{ location: "/fixture/z.txt", name: "z.txt", is_dir: false }]
            : [
                { location: "/fixture/a", name: "a", is_dir: true },
                { location: "/fixture/b", name: "b", is_dir: true },
              ],
          truncated: false,
        };
      }
      return {
        location: params.location,
        name: "z.txt",
        text: "cat content",
        truncated: false,
        binary: false,
      };
    });
    const result = await index.search(
      { ...request(directory, "search"), prompt: "feline" },
      readConfig({
        embedding_endpoint: url,
        embedding_model: "fixture",
        max_files: 1,
      }),
      context,
    );
    assert.equal(result.receipt.files, 1);
    assert.equal(result.files[0]?.name, "z.txt");
    index.close();
  }));
