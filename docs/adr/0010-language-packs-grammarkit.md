# ADR 0010 — Language packs with Grammar-Kit and JFlex files

Status: Accepted (user request), 2026-10-21

## Context
The flexible-* plugins contain Grammar-Kit BNF grammars, JFlex lexers, and Java/Kotlin logic
for languages that tree-sitter covers badly or not at all (COBOL, Delphi, ABAP, and others).
23 plugins use about 158 IntelliJ extension points. The user wants to port grammars and logic
with little work.

## Decision
- The **AIVory Kit** engine (a future crate, not built) reads `.bnf` and `.flex` files without changes. It
  generates Rust parser and lexer code at build time. It interprets them at run time for the
  grammar workbench.
- A **pack** = grammar files + `pack.toml` (declarative features) + queries + an optional WASM
  component for logic.
- The logic API ("PSI-lite", WIT) uses the IntelliJ names: `PsiElement`, `PsiReference.resolve`,
  `Annotator`, `LocalInspectionTool`, `FoldingBuilder`, `DocumentationProvider`,
  `CompletionContributor`.
- Conformance: the PSI dumps of the plugins' parser tests must match.

## Rejected alternatives
- **Convert BNF to tree-sitter grammar.js:** loses `pin` and `recoverWhile` recovery and the
  PSI shape that the logic expects.
- **Run the plugins in a JVM sidecar:** start-up and memory cost; IntelliJ platform dependency.
- **Rewrite each language in tree-sitter:** loses years of grammar work.

## Consequences
- Java actions in JFlex files and `external` rules need a supported subset or a Rust/WASM port.
  The Kit linter reports each unsupported construct.
- A pack can also generate a JetBrains plugin skeleton, so one grammar serves both products.
