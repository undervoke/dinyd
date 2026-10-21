# Blueprint Diagrams

- `.mermaid` only; MPE `@import` treats `.mmd` as plain code. Palette targets the VS Code dark background `#1f1f1f`.
- Diagrams render in both the Mermaid Preview extension (`@mermaid-chart/mermaid` 11.16.1-b.0), which opens `.mermaid` files, and MPE (Mermaid 12.0.0), which renders the `@import`s. Choose diagram types and syntax supported by both preview engines.
- Leading `%%` names the behavior authority.
- Node: `heading<br/><small>detail</small>`. Two or more distinct statements: one `• ` line each; one `A → B → C` chain stays one line.
- Achromatic by default; use a type's native dark palette when its labels, lines, and series remain distinguishable on the target background. Color marks `em` (focus), `issue` (observed defect), `inactive` (superseded or suspended), and infrastructure kinds `db` · `cache` · `queue` · `file` · `ext`; chart categories and series take the infrastructure hues in the example's order, so green and red keep meaning `em` and `issue`. No per-node `style`, so palette edits stay in `classDef` and frontmatter.
- Copy frontmatter and `classDef`s from the type's example; its `%%` comments hold that type's rules.

## Flowchart

@import "flowchart.mermaid"

## Sequence

@import "sequence.mermaid"

## State

@import "state.mermaid"

## ER

@import "er.mermaid"

## Class

@import "class.mermaid"

## C4

@import "c4-context.mermaid"

@import "c4.mermaid"

@import "c4-component.mermaid"

@import "c4-dynamic.mermaid"

@import "c4-deployment.mermaid"

## Gantt

@import "gantt.mermaid"

## Pie

@import "pie.mermaid"

## Requirement

@import "requirement.mermaid"

## Git graph

@import "gitgraph.mermaid"

## User journey

@import "journey.mermaid"

## Quadrant

@import "quadrant.mermaid"

## Sankey

@import "sankey.mermaid"

## Packet

@import "packet.mermaid"

## XY chart

@import "xychart.mermaid"

## Block

@import "block.mermaid"

## Timeline

@import "timeline.mermaid"

## Mindmap

@import "mindmap.mermaid"

## Kanban

@import "kanban.mermaid"

## Architecture

@import "architecture.mermaid"

## Radar

@import "radar.mermaid"

## Treemap

@import "treemap.mermaid"

## Tree view

@import "treeview.mermaid"

## Ishikawa

@import "ishikawa.mermaid"

## Event modeling

@import "eventmodeling.mermaid"

## Venn

@import "venn.mermaid"

## Wardley

@import "wardley.mermaid"

## Cynefin

@import "cynefin.mermaid"

## Swimlanes

@import "swimlanes.mermaid"

## Railroad

@import "railroad.mermaid"

@import "railroad-abnf.mermaid"

@import "railroad-peg.mermaid"

@import "railroad-ir.mermaid"
