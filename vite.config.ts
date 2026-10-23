import { defineConfig } from "vite-plus";

export default defineConfig({
  fmt: {
    printWidth: 100,
    sortImports: false,
    ignorePatterns: ["**/*", "!**/", "!**/*.{ts,tsx,mts,cts}", "**/dist/**", "**/coverage/**"],
  },
  lint: {
    env: { node: true },
    options: { typeAware: true, typeCheck: true },
    ignorePatterns: ["**/dist/**", "**/coverage/**", "**/*.{js,jsx,mjs,cjs}"],
  },
  test: {
    projects: ["sdk/typescript", "plugins/ai"],
    maxWorkers: 2,
  },
});
