import { defineConfig } from "vite-plus";

export default defineConfig({
  run: {
    tasks: {
      bundle: {
        command: "vp pack",
        cache: false,
      },
    },
  },
  pack: {
    entry: ["src/index.ts"],
    format: "esm",
    platform: "node",
    target: "es2023",
    fixedExtension: false,
    dts: true,
    minify: false,
  },
  test: {
    name: "plugin-sdk",
    include: ["test/**/*.test.ts"],
    environment: "node",
    pool: "forks",
    testTimeout: 10000,
  },
});
