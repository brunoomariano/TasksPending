import { defineConfig } from "vite";

export default defineConfig({
  server: {
    proxy: {
      "/api": "http://127.0.0.1:61000",
      "/healthz": "http://127.0.0.1:61000",
    },
  },
});
