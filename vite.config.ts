import { defineConfig } from "vitest/config";

const page = (name: string) => new URL(name, import.meta.url).pathname;

// Two pages: the radar overlay and the settings window.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    target: "es2022",
    outDir: "dist",
    rollupOptions: {
      input: {
        radar: page("index.html"),
        settings: page("settings.html"),
      },
    },
  },
  test: {
    include: ["src/**/*.test.ts"],
  },
});
