// 控制台编译:src/main.ts + src/style.css → 单文件 dist/index.html。
// 运行时契约不变(index.cgi 服务 target/www/index.html + .gz/.br 预压缩),
// 变化的只是"源码可编译":语法错误在此处即失败,不再静默上线(r7 教训)。
import { build } from "esbuild";
import { readFileSync, writeFileSync, mkdirSync } from "fs";

const buildId = process.env.BUILD_ID || "dev";
mkdirSync("dist", { recursive: true });

await build({
  entryPoints: ["src/main.ts"],
  bundle: true,
  outfile: "dist/main.js",
  minify: true,
  target: ["es2019"], // fnOS 桌面/Android WebView 均为 Chromium,es2019 富余
  charset: "utf8",
  logLevel: "warning",
});

await build({
  entryPoints: ["src/style.css"],
  outfile: "dist/main.css",
  minify: true,
  logLevel: "silent",
});

const tpl = readFileSync("src/index.html", "utf8");
const js = readFileSync("dist/main.js", "utf8");
const css = readFileSync("dist/main.css", "utf8");

// minified 代码可能含 "</script>"(字符串拼接),内联前必须转义;
// 用函数形式替换,避免 $&/$` 等替换模式语义
const safeJs = js.replace(/<\/script>/g, () => "<\\/script>");
const safeCss = css.replace(/<\/style>/g, () => "<\\/style>");
const html = tpl
  .replace("/*__INLINE_CSS__*/", () => safeCss)
  .replace("//__INLINE_JS__", () => safeJs)
  .replace("__BUILD_ID__", () => buildId);

writeFileSync("dist/index.html", html);
console.log(`dist/index.html: ${(html.length / 1024).toFixed(1)} KB (build ${buildId})`);
