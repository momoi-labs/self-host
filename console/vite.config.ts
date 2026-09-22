import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const { version } = JSON.parse(
  readFileSync(resolve(import.meta.dirname, "../package.json"), "utf8"),
) as { version: string };

// The Platform serves the console from /console/: /console is login, /console/
// is the application, and setup.html is the public page a device sees over
// plain HTTP before it can log in.
export default defineConfig({
  base: "/console/",
  define: {
    "import.meta.env.SELF_HOST_VERSION": JSON.stringify(version),
  },
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      input: {
        index: resolve(import.meta.dirname, "index.html"),
        login: resolve(import.meta.dirname, "login.html"),
        setup: resolve(import.meta.dirname, "setup.html"),
      },
    },
  },
  server: {
    // `npm run dev` talks to a `self-host serve` on the default port, so the
    // console can be developed against real applications.
    proxy: Object.fromEntries(
      [
        "/health",
        "/events",
        "/environments",
        "/apps",
        "/system",
        "/bootstrap",
        "/compose",
        "/api-keys",
        "/settings",
        "/custom-images",
        "/metrics",
      ].map((path) => [
        path,
        { target: "http://127.0.0.1:3721", changeOrigin: true, ws: true },
      ]),
    ),
  },
});
