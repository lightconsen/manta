import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// base is the repo name so GitHub Pages (project site) resolves assets:
// https://lightconsen.github.io/syscity/
export default defineConfig({
  plugins: [react()],
  base: "/syscity/",
});
