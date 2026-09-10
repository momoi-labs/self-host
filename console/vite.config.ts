import { resolve } from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The Platform serves the console from /console/, and the four pages keep the
// URLs they already had: /console is login, /console/ is the application, and
// the two settings pages stay linkable on their own.
export default defineConfig({
  base: "/console/",
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      input: {
        index: resolve(import.meta.dirname, "index.html"),
        login: resolve(import.meta.dirname, "login.html"),
        setup: resolve(import.meta.dirname, "setup.html"),
        "api-keys": resolve(import.meta.dirname, "api-keys.html"),
      },
    },
  },
  server: {
    // `npm run dev` talks to a `self-host serve` on the default port, so the
    // console can be developed against real applications.
    proxy: Object.fromEntries(
      [
        "/health",
        "/apps",
        "/system",
        "/bootstrap",
        "/compose",
        "/api-keys",
        "/dev-images",
        "/metrics",
      ].map((path) => [
        path,
        { target: "http://127.0.0.1:3721", changeOrigin: true, ws: true },
      ]),
    ),
  },
});
