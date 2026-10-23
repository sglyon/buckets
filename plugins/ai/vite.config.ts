import { defineConfig } from "vite-plus";

export default defineConfig({
  run: {
    tasks: {
      bundle: {
        command: "vp pack",
        dependsOn: ["@buckets/plugin-sdk#bundle"],
        cache: false,
      },
    },
  },
  pack: {
    entry: ["src/*.ts"],
    root: "src",
    format: "esm",
    platform: "node",
    target: "es2023",
    fixedExtension: false,
    unbundle: true,
    dts: false,
    minify: false,
    treeshake: false,
  },
  test: {
    name: "workspace-ai",
    include: ["test/**/*.test.ts"],
    environment: "node",
    pool: "forks",
    testTimeout: 10000,
  },
});
