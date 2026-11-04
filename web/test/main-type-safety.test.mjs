import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const sourcePath = fileURLToPath(new URL("../src/main.ts", import.meta.url));

function findForbiddenTypeScript(source) {
  const sourceFile = ts.createSourceFile(sourcePath, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const violations = [];

  function visit(node) {
    if (node.kind === ts.SyntaxKind.NonNullExpression) {
      violations.push("non-null assertion");
    }
    if (node.kind === ts.SyntaxKind.AnyKeyword) {
      violations.push("any type");
    }
    if (node.kind === ts.SyntaxKind.AsExpression) {
      if (node.type.kind === ts.SyntaxKind.AnyKeyword) {
        violations.push("as any");
      }
      if (node.expression.kind === ts.SyntaxKind.AsExpression
        || node.expression.kind === ts.SyntaxKind.TypeAssertionExpression) {
        violations.push("double assertion");
      }
    }
    if (node.kind === ts.SyntaxKind.TypeAssertionExpression) {
      if (node.type.kind === ts.SyntaxKind.AnyKeyword) {
        violations.push("any type assertion");
      }
      if (node.expression.kind === ts.SyntaxKind.AsExpression
        || node.expression.kind === ts.SyntaxKind.TypeAssertionExpression) {
        violations.push("double assertion");
      }
    }
    ts.forEachChild(node, visit);
  }

  visit(sourceFile);
  if (/\/\/\s*@ts-ignore\b/.test(source)) violations.push("@ts-ignore");
  return violations;
}

test("main.ts contains no unsafe TypeScript assertions or any types", async () => {
  const source = await readFile(sourcePath, "utf8");
  assert.deepEqual(findForbiddenTypeScript(source), []);
});
