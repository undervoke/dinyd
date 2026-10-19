# Blueprint

dinyd's design. Each diagram owns one axis, and its `%%` comments hold the behavior authority.

dinyd ties a claim in a document to the code region that is its evidence. When code changes and a reference drifts, dinyd leads the worker (person · agent) to open the document and designate the reference, and only that updates it. Reading and judging are the worker's; dinyd does not deliver document content in its output. dinyd provides only the verdict and update logic, and the consuming project decides where to call it.

## Diagrams

- `.mermaid` only; MPE `@import` treats `.mmd` as plain code. Palette targets the VS Code dark background `#1f1f1f`.
- Leading `%%` names the behavior authority.
- English text, identifiers verbatim. Frontmatter `title`; group titles instead of legends.
- Node: `heading<br/><small>detail</small>`. Two or more distinct statements: one `• ` line each; one `A → B → C` chain stays one line.
- Achromatic by default. Color only for `em` (focus), `issue` (observed defect), `legacy` (superseded), and infrastructure kinds `db` · `cache` · `queue` · `file` · `ext`. No per-node `style`, so palette edits stay in `classDef` and frontmatter.
- Copy frontmatter and `classDef`s from an existing diagram of the same type.

## anchors

The two kinds of references a document holds (digest anchor · delegation), per-format selectors, document units, and digest modes.

@import "anchors.mermaid"

## check

Verdicts: for each anchor, find the target and compare digests to separate ok from failures, and set the worker's next step for each failure.

@import "check.mermaid"

## apply-update

Creating and updating relationships: when the worker opens the document and designates the claim line and its evidence, apply writes the anchor and first digest; after a check failure, update rewrites the digest.

@import "apply-update.mermaid"

## distribution

Build · release, and the boundary where a consuming project runs dinyd.

@import "distribution.mermaid"

## Extensions under consideration

To review after the first release: impact propagation along call relations, a reverse index, JSON output, a resident process, graph visualization.
