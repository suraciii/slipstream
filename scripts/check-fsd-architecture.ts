import ts from "typescript";
import { existsSync } from "node:fs";
import { basename, dirname, relative, resolve, sep } from "node:path";

const LAYERS = ["app", "pages"] as const;
const SEGMENTS = new Set(["api", "config", "lib", "model", "ui"]);
const IMPORT_EXTENSIONS = new Set([".ts", ".tsx", ".mts", ".cts"]);

type Diagnostic = Readonly<{ file: string; line: number; message: string }>;
type Edge = Readonly<{
  from: string;
  to: string;
  node: ts.Node;
  typeOnly: boolean;
}>;

const normalize = (file: string): string => resolve(file).split(sep).join("/");
const isCodeImport = (specifier: string): boolean => {
  const name = specifier.slice(
    Math.max(specifier.lastIndexOf("/"), specifier.lastIndexOf("\\")) + 1,
  );
  const extension = name.includes(".") ? name.slice(name.lastIndexOf(".")) : "";
  return !extension || IMPORT_EXTENSIONS.has(extension) || extension === ".js";
};
const isWithin = (file: string, root: string): boolean => {
  const rel = relative(root, file);
  return (
    rel === "" ||
    (!rel.startsWith("..") &&
      !rel.startsWith(`..${sep}`) &&
      !rel.includes(`..${sep}`))
  );
};
const display = (file: string, root: string): string =>
  relative(root, file).split(sep).join("/");

function sourceInfo(
  file: string,
  sourceRoot: string,
): { layer?: string; slice?: string; segment?: string } {
  const rel = relative(sourceRoot, file).split(sep);
  if (rel.length < 2 || !LAYERS.includes(rel[0] as (typeof LAYERS)[number]))
    return {};
  const layer = rel[0];
  const slice = layer === "app" ? undefined : rel[1];
  const rawSegment =
    layer === "app"
      ? rel[1]?.replace(/\.[^.]+$/, "")
      : rel[2]?.replace(/\.[^.]+$/, "");
  const segment =
    rawSegment && !["index", "page"].includes(rawSegment)
      ? rawSegment
      : undefined;
  return { layer, slice, segment };
}

function isTypeOnlyImport(node: ts.Node): boolean {
  if (ts.isImportDeclaration(node)) {
    return (
      Boolean(node.importClause?.isTypeOnly) ||
      Boolean(
        !node.importClause?.name &&
          node.importClause?.namedBindings &&
          ts.isNamedImports(node.importClause.namedBindings) &&
          node.importClause.namedBindings.elements.length > 0 &&
          node.importClause.namedBindings.elements.every((e) => e.isTypeOnly),
      )
    );
  }
  if (ts.isExportDeclaration(node)) {
    return (
      node.isTypeOnly ||
      Boolean(
        node.exportClause &&
          ts.isNamedExports(node.exportClause) &&
          node.exportClause.elements.length > 0 &&
          node.exportClause.elements.every((e) => e.isTypeOnly),
      )
    );
  }
  return false;
}

function collectEdges(
  source: ts.SourceFile,
  program: ts.Program,
  diagnostics: Diagnostic[],
  root: string,
): Edge[] {
  const checker = program.getCompilerOptions();
  const edges: Edge[] = [];
  const add = (node: ts.Node, text: string, typeOnly: boolean): void => {
    if (!isCodeImport(text)) return;
    const resolved = ts.resolveModuleName(
      text,
      source.fileName,
      checker,
      ts.sys,
    ).resolvedModule;
    if (!resolved) {
      const aliases = checker.paths
        ? Object.keys(checker.paths).some((pattern) =>
            pattern.endsWith("/*")
              ? text.startsWith(pattern.slice(0, -1))
              : text === pattern,
          )
        : false;
      const internal =
        text.startsWith(".") ||
        text.startsWith("/") ||
        text.startsWith("@/") ||
        text.startsWith("~/") ||
        aliases;
      if (internal) {
        const pos = source.getLineAndCharacterOfPosition(node.getStart(source));
        diagnostics.push({
          file: display(source.fileName, root),
          line: pos.line + 1,
          message: `unresolved internal import ${JSON.stringify(text)}`,
        });
      }
      return;
    }
    const target = normalize(resolved.resolvedFileName);
    if (isWithin(target, root))
      edges.push({
        from: normalize(source.fileName),
        to: target,
        node,
        typeOnly,
      });
  };
  const visit = (node: ts.Node): void => {
    if (
      ts.isImportDeclaration(node) &&
      ts.isStringLiteral(node.moduleSpecifier)
    )
      add(node, node.moduleSpecifier.text, isTypeOnlyImport(node));
    else if (
      ts.isExportDeclaration(node) &&
      node.moduleSpecifier &&
      ts.isStringLiteral(node.moduleSpecifier)
    )
      add(node, node.moduleSpecifier.text, isTypeOnlyImport(node));
    else if (
      ts.isImportEqualsDeclaration(node) &&
      ts.isExternalModuleReference(node.moduleReference) &&
      ts.isStringLiteral(node.moduleReference.expression)
    )
      add(node, node.moduleReference.expression.text, false);
    else if (
      ts.isCallExpression(node) &&
      node.expression.kind === ts.SyntaxKind.ImportKeyword &&
      node.arguments.length === 1 &&
      ts.isStringLiteral(node.arguments[0])
    )
      add(node, node.arguments[0].text, false);
    ts.forEachChild(node, visit);
  };
  visit(source);
  return edges;
}

function findCycles(edges: Edge[]): Edge[][] {
  const byFrom = new Map<string, Edge[]>();
  for (const edge of edges)
    if (!edge.typeOnly)
      byFrom.set(edge.from, [...(byFrom.get(edge.from) ?? []), edge]);
  const color = new Map<string, 0 | 1 | 2>();
  const stack: Edge[] = [];
  const found: Edge[][] = [];
  const visit = (file: string): void => {
    color.set(file, 1);
    for (const edge of byFrom.get(file) ?? []) {
      const targetColor = color.get(edge.to) ?? 0;
      if (targetColor === 1) {
        const index =
          edge.to === file
            ? stack.length
            : stack.findIndex((item) => item.from === edge.to);
        found.push([...stack.slice(index < 0 ? 0 : index), edge]);
      } else if (targetColor === 0) {
        stack.push(edge);
        visit(edge.to);
        stack.pop();
      }
    }
    color.set(file, 2);
  };
  for (const file of byFrom.keys())
    if ((color.get(file) ?? 0) === 0) visit(file);
  return found;
}

export function checkFsdArchitecture(
  projectRoot = process.cwd(),
): Diagnostic[] {
  const root = normalize(projectRoot);
  const webRoot = resolve(root, "apps/web");
  const sourceRoot = resolve(webRoot, "src");
  const tsconfig = resolve(webRoot, "tsconfig.json");
  if (!existsSync(tsconfig))
    return [
      {
        file: "apps/web/tsconfig.json",
        line: 1,
        message: "Web tsconfig.json is required for architecture resolution",
      },
    ];
  const configFile = ts.readConfigFile(tsconfig, (path) =>
    ts.sys.readFile(path),
  );
  if (configFile.error)
    return [
      {
        file: "apps/web/tsconfig.json",
        line: 1,
        message: ts.flattenDiagnosticMessageText(
          configFile.error.messageText,
          " ",
        ),
      },
    ];
  const parsed = ts.parseJsonConfigFileContent(
    configFile.config,
    ts.sys,
    dirname(tsconfig),
  );
  const rootNames = ts.sys.readDirectory(sourceRoot, [".ts"]);
  const program = ts.createProgram({ rootNames, options: parsed.options });
  const diagnostics: Diagnostic[] = [];
  const sourceFiles = program
    .getSourceFiles()
    .filter(
      (file) =>
        isWithin(normalize(file.fileName), sourceRoot) &&
        !file.isDeclarationFile,
    );
  const analyzed = sourceFiles.filter((file) => {
    const rel = relative(sourceRoot, file.fileName).split(sep);
    return (
      (rel[0] === "app" || rel[0] === "pages") &&
      !/\.(test|browser-test)\.ts$/.test(file.fileName)
    );
  });
  for (const file of sourceFiles) {
    const rel = relative(sourceRoot, file.fileName).split(sep);
    if (
      rel.length === 1 &&
      /\.tsx?$/.test(rel[0]) &&
      !/\.(test|browser-test)\.ts$/.test(rel[0]) &&
      rel[0] !== "browser-server.ts"
    )
      diagnostics.push({
        file: display(file.fileName, root),
        line: 1,
        message:
          "runtime source must live under app/ or pages/ (obsolete root UI file)",
      });
    if (
      rel[0] === "pages" &&
      rel.length >= 3 &&
      !SEGMENTS.has(rel[2].replace(/\.[^.]+$/, "")) &&
      !["index", "page"].includes(rel[2].replace(/\.[^.]+$/, ""))
    )
      diagnostics.push({
        file: display(file.fileName, root),
        line: 1,
        message: `invalid page segment ${JSON.stringify(rel[2].replace(/\.[^.]+$/, ""))}; use api, config, lib, model, or ui`,
      });
  }
  const edges = analyzed.flatMap((file) =>
    collectEdges(file, program, diagnostics, root),
  );
  const byFile = new Map(
    analyzed.map((file) => [normalize(file.fileName), file]),
  );
  for (const edge of edges) {
    const from = sourceInfo(edge.from, sourceRoot);
    const to = sourceInfo(edge.to, sourceRoot);
    if (!from.layer || !to.layer) continue;
    const source = byFile.get(edge.from);
    if (!source) continue;
    if (from.layer === "app" && to.layer === "app") {
      /* app segments may compose each other */
    }
    if (from.layer === "app" && to.layer === "pages") {
      const rel = relative(resolve(sourceRoot, "pages"), edge.to).split(sep);
      if (rel.length < 2 || rel[1] !== "index.ts")
        diagnostics.push({
          file: display(edge.from, root),
          line:
            source.getLineAndCharacterOfPosition(edge.node.getStart(source))
              .line + 1,
          message: "app imports a page only through its public index.ts API",
        });
    }
    if (from.layer === "pages" && to.layer === "app")
      diagnostics.push({
        file: display(edge.from, root),
        line:
          source.getLineAndCharacterOfPosition(edge.node.getStart(source))
            .line + 1,
        message: "pages must not import from app",
      });
    if (
      from.layer === "pages" &&
      to.layer === "pages" &&
      from.slice !== to.slice
    )
      diagnostics.push({
        file: display(edge.from, root),
        line:
          source.getLineAndCharacterOfPosition(edge.node.getStart(source))
            .line + 1,
        message: `same-layer slices must not import each other (${from.slice} -> ${to.slice})`,
      });
    if (
      from.layer === "pages" &&
      from.slice === to.slice &&
      to.segment === undefined &&
      basename(edge.to) === "index.ts"
    )
      diagnostics.push({
        file: display(edge.from, root),
        line:
          source.getLineAndCharacterOfPosition(edge.node.getStart(source))
            .line + 1,
        message:
          "files inside a slice must not import their own public index.ts",
      });
  }
  for (const cycle of findCycles(edges))
    diagnostics.push({
      file: display(cycle[0].from, root),
      line: 1,
      message: `runtime import cycle: ${cycle.map((edge) => `${display(edge.from, root)} -> ${display(edge.to, root)}`).join("; ")}`,
    });
  return diagnostics;
}

if (import.meta.main) {
  const diagnostics = checkFsdArchitecture(process.argv[2] ?? process.cwd());
  if (diagnostics.length) {
    for (const diagnostic of diagnostics)
      console.error(
        `${diagnostic.file}:${diagnostic.line}: ${diagnostic.message}`,
      );
    process.exitCode = 1;
  } else console.log("FSD architecture gate passed");
}
