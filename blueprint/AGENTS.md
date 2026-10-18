# Blueprint

dinyd의 설계. 각 다이어그램이 하나의 축을 소유하고, 동작 정본은 각 `%%` 주석이 담는다.

dinyd는 문서의 claim과 그 근거인 코드 영역을 잇는다. 코드가 바뀌어 참조가 어긋나면 작업자(사람 · agent)가 문서를 직접 열어 참조를 지정해야만 갱신되도록 유도한다. 읽고 판단하는 것은 작업자의 몫이고, dinyd는 문서 내용을 출력으로 전달하지 않는다. dinyd는 판정과 갱신 로직만 제공하고, 어디서 호출할지는 사용하는 프로젝트가 정한다.

## Diagrams

- `.mermaid` only; MPE `@import` treats `.mmd` as plain code. Palette targets the VS Code dark background `#1f1f1f`.
- Leading `%%` names the behavior authority.
- Korean text, identifiers verbatim. Frontmatter `title`; group titles instead of legends.
- Node: `heading<br/><small>detail</small>`. Two or more distinct statements: one `• ` line each; one `A → B → C` chain stays one line.
- Achromatic by default. Color only for `em` (focus), `issue` (observed defect), `legacy` (superseded), and infrastructure kinds `db` · `cache` · `queue` · `file` · `ext`. No per-node `style`, so palette edits stay in `classDef` and frontmatter.
- Copy frontmatter and `classDef`s from an existing diagram of the same type.

## anchors

문서에 두는 참조 두 종류(digest anchor · 위임 참조), 형식별 selector, digest 방식.

@import "anchors.mermaid"

## check

판정: anchor마다 대상을 찾고 digest를 비교해 ok와 실패를 가르고, 실패마다 작업자가 할 일을 정한다.

@import "check.mermaid"

## apply-update

관계 만들기와 갱신: 작업자가 문서를 열어 claim 줄과 근거를 지정하면 apply가 anchor와 첫 digest를 쓰고, check 실패 뒤에는 update가 digest를 다시 쓴다.

@import "apply-update.mermaid"

## distribution

빌드 · 배포와 사용하는 프로젝트가 dinyd를 실행하는 경계.

@import "distribution.mermaid"

## 확장 고려

첫 release 다음에 검토한다: 호출 관계를 따라가는 영향 전파, reverse index, JSON 출력, 상주 프로세스, graph 시각화.
