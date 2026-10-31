import { build } from "esbuild";
import solidPlugin from "esbuild-plugin-solid";
await build({
  entryPoints: ["src/entry-solid.tsx"],
  bundle: true,
  outfile: "dist/solid-probe.js",
  minify: true,
  target: ["es2020"],
  charset: "utf8",
  plugins: [solidPlugin()],
  logLevel: "warning",
});
console.log("probe built");
