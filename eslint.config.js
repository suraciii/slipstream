import eslint from "@eslint/js";
import prettier from "eslint-config-prettier";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    ignores: [
      "**/dist/**",
      "**/coverage/**",
      "**/*-worker.cjs",
      "**/file-kinds.cjs",
      "eslint.config.js",
    ],
  },
  eslint.configs.recommended,
  ...tseslint.configs.recommendedTypeChecked,
  prettier,
  {
    files: ["apps/**/*.ts", "scripts/**/*.ts", "playwright.config.ts"],
    languageOptions: {
      globals: globals.node,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
  },
  {
    files: ["apps/web/src/**/*.ts"],
    ignores: ["apps/web/src/pages/**/*.ts"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              regex: "^(?:\\.\\.?/)+(?:src/)?pages/[^/]+(?:$|/(?!index\\.js$))",
              message: "Import a page through its public index.ts API.",
            },
          ],
        },
      ],
    },
  },
  {
    files: ["apps/web/src/pages/**/*.ts"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              regex: "^(?:\\.\\.?/)+(?:src/)?app(?:/|$)",
              message: "A page must not import from the app layer.",
            },
          ],
        },
      ],
    },
  },
);
