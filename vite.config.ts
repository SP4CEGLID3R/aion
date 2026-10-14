import { defineConfig } from "vitest/config";

const page = (name: string) => new URL(name, import.meta.url).pathname;

// Three pages: the radar overlay, the settings window and the arrow around
// the character.
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
        guide: page("guide.html"),
      },
    },
  },
  test: {
    include: ["src/**/*.test.ts"],
  },
});
