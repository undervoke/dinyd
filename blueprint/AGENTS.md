# Blueprint

State each point once and link to its owner elsewhere.

## Layers

- Documents own decisions, rationale, and intended boundaries that cannot be inferred from code.
- Diagrams explain cross-file and process relationships at an abstraction level that avoids copying code literals.
- Code owns implementation facts; link to their source instead of maintaining parallel inventories or configuration values.
- Topic documents import their diagrams.
- Keep each independently reviewable claim on its own physical line.

## Diagrams

- `.mermaid` only; MPE `@import` treats `.mmd` as plain code.
- A single leading `%%` names the owning document; additional comments are for rendering only.
- Copy frontmatter and `classDef`s from the type's example in [diagram-examples](diagram-examples/); its rendering comments and on-canvas text explain the type's conventions.
- Color only through those `classDef`s, never per-node `style`, so palette edits stay in `classDef` and frontmatter.
- The palette targets the VS Code dark background `#1f1f1f`.
