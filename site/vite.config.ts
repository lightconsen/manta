import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Relative base so assets resolve under any deployment root: the custom
// domain (https://syscity.net/, served at "/") and the project site
// (https://lightconsen.github.io/syscity/). The site is a single static
// landing page with no client-side routing, so relative paths are safe.
export default defineConfig({
  plugins: [react()],
  base: "./",
});
