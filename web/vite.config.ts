import { defineConfig } from "vite";

export default defineConfig({
  build: {
    outDir: "dist",
  },
  server: {
    proxy: {
      "/api": {
        target: "http://127.0.0.1:8099",
        changeOrigin: true,
      },
    },
  },
});
