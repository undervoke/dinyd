//! Drives the dinyd binary against real temporary Git repositories.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

struct Repo {
    dir: TempDir,
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn lines(&self) -> Vec<&str> {
        self.stdout.lines().collect()
    }
}

impl Repo {
    fn new() -> Repo {
        let repo = Repo {
            dir: TempDir::new().unwrap(),
        };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        repo
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, path: &str, content: &str) {
        let full = self.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.path().join(path)).unwrap()
    }

    fn git(&self, args: &[&str]) -> String {
        let output = self.git_output(args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn git_output(&self, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(self.path())
            .output()
            .unwrap()
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    fn dinyd(&self, args: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_dinyd"))
            .args(args)
            .current_dir(self.path())
            .output()
            .unwrap();
        Run {
            code: output.status.code().unwrap(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }

    /// Runs apply and returns the anchor text it wrote.
    fn apply(&self, doc: &str, line: usize, selector: &str, extra: &[&str]) -> String {
        let line = line.to_string();
        let mut args = vec!["apply", doc, "--line", &line, "--selector", selector];
        args.extend(extra);
        let run = self.dinyd(&args);
        assert_eq!(run.code, 0, "apply {selector}: {}", run.stderr);
        run.stdout.trim().to_string()
    }

    fn check(&self, args: &[&str]) -> Run {
        let mut all = vec!["check"];
        all.extend(args);
        self.dinyd(&all)
    }
}

const STALE: &str = "stale — re-judge the claim, then update";
const UNDECLARED: &str = "undeclared — fill the digest with update";
const UNRESOLVED: &str = "unresolved — re-judge the claim and the reference";
const MALFORMED: &str = "malformed — re-judge the claim and the reference";

const CLASS: &str = "class A {\n  void F() { }\n  void G() { }\n}\n";

#[test]
fn five_verdicts_under_each_basis() {
    let repo = Repo::new();
    repo.write("src/a.cs", CLASS);
    repo.write(
        "doc.md",
        "# Doc\n\nF does nothing.\n\nG does nothing.\n\nF again <!-- dinyd: src/a.cs#A.F -->\n\nGone <!-- dinyd-ref: src/missing.cs -->\n\nBad <!-- dinyd: ../a.cs -->\n",
    );
    repo.git(&["add", "-A"]);
    repo.apply("doc.md", 3, "src/a.cs#A.F", &[]);
    let old_g = repo.apply("doc.md", 5, "src/a.cs#A.G", &[]);
    repo.write(
        "src/a.cs",
        &CLASS.replace("void G() { }", "void G() { return; }"),
    );
    repo.commit("state");

    let expected = vec![
        format!("doc.md → src/a.cs#A.G: {STALE}"),
        format!("doc.md → src/a.cs#A.F: {UNDECLARED}"),
        format!("doc.md → src/missing.cs: {UNRESOLVED}"),
        format!("doc.md → ../a.cs: {MALFORMED}"),
    ];
    for basis in [&["--index"][..], &["--worktree"], &["--rev", "HEAD"], &[]] {
        let run = repo.check(basis);
        assert_eq!(run.code, 1, "{basis:?}");
        assert_eq!(run.lines(), expected, "{basis:?}");
        // Output names the doc and the selector only: no line numbers, digests, or claim text.
        let digest = old_g.rsplit(':').next().unwrap();
        for output in [&run.stdout, &run.stderr] {
            assert!(!output.contains(digest));
            assert!(
                !output.contains("does nothing")
                    && !output.contains("again")
                    && !output.contains("Gone")
            );
            assert!(!output.contains(":3") && !output.contains(":5") && !output.contains("line"));
        }
    }

    repo.write("doc.md", "# Doc\n\nF does nothing.\n");
    repo.git(&["add", "-A"]);
    repo.apply("doc.md", 3, "src/a.cs#A.F", &[]);
    repo.git(&["add", "-A"]);
    let run = repo.check(&[]);
    assert_eq!((run.code, run.stdout.as_str()), (0, ""));
}

#[test]
fn each_basis_reads_its_own_content() {
    let repo = Repo::new();
    repo.write("src/a.cs", CLASS);
    repo.write("doc.md", "F does nothing.\n");
    repo.git(&["add", "-A"]);
    repo.apply("doc.md", 1, "src/a.cs#A.F", &[]);
    repo.commit("v1");

    // Partial staging: the index holds v2, the working tree v3.
    repo.write(
        "src/a.cs",
        &CLASS.replace("void F() { }", "void F() { int v2; }"),
    );
    repo.git(&["add", "src/a.cs"]);
    repo.write(
        "src/a.cs",
        &CLASS.replace("void F() { }", "void F() { int v3; }"),
    );

    assert_eq!(repo.check(&["--rev", "HEAD"]).code, 0);
    assert_eq!(
        repo.check(&["--index"]).lines(),
        [format!("doc.md → src/a.cs#A.F: {STALE}")]
    );
    assert_eq!(repo.check(&["--worktree"]).code, 1);

    let run = repo.dinyd(&[
        "update",
        "doc.md",
        "--line",
        "1",
        "--expect-selector",
        "src/a.cs#A.F",
        "--worktree",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(repo.check(&["--worktree"]).code, 0);
    // The updated doc is not staged, and the index target differs from the working tree.
    assert_eq!(repo.check(&["--index"]).code, 1);
    repo.git(&["add", "-A"]);
    assert_eq!(repo.check(&["--index"]).code, 0);
    assert_eq!(
        repo.check(&["--rev", "HEAD"]).code,
        0,
        "HEAD still pairs the v1 doc with v1 code"
    );
}

struct Kind {
    path: &'static str,
    content: &'static str,
    selector: &'static str,
    outside: Option<(&'static str, &'static str)>,
    inside: (&'static str, &'static str),
}

#[test]
fn every_selector_kind_tracks_only_its_region() {
    let kinds = [
        Kind {
            path: "notes.txt",
            content: "plain notes\n",
            selector: "notes.txt",
            outside: None,
            inside: ("plain", "other"),
        },
        Kind {
            path: "cfg.json",
            content: "{\"a\": {\"b\": [1, 2]}, \"c\": 3}\n",
            selector: "cfg.json#/a/b/1",
            outside: Some(("\"c\": 3", "\"c\": 4")),
            inside: ("2]", "5]"),
        },
        Kind {
            path: "cfg.yaml",
            content: "a:\n  b:\n    - x\n    - y\nc: 1\n",
            selector: "cfg.yaml#/a/b/1",
            outside: Some(("c: 1", "c: 2")),
            inside: ("- y", "- z"),
        },
        Kind {
            path: "site.css",
            content: ".a .b { color: red; }\n.c { color: blue; }\n",
            selector: "site.css#.a .b",
            outside: Some(("blue", "green")),
            inside: ("red", "black"),
        },
        Kind {
            path: "src/a.cs",
            content: "namespace N;\nclass A { void F() { } void G() { } }\n",
            selector: "src/a.cs#A.F",
            outside: Some(("G() { }", "G() { return; }")),
            inside: ("F() { }", "F() { return; }"),
        },
        Kind {
            path: "src/c.ts",
            content: "export class C {\n  m(): number { return 1; }\n  n() {}\n}\n",
            selector: "src/c.ts#C.m",
            outside: Some(("n() {}", "n() { return; }")),
            inside: ("return 1", "return 2"),
        },
        Kind {
            path: "src/f.js",
            content: "export function f() { return 1; }\nexport function g() { return 2; }\n",
            selector: "src/f.js#f",
            outside: Some(("return 2", "return 3")),
            inside: ("return 1", "return 4"),
        },
        Kind {
            path: "src/t.go",
            content: "package p\n\ntype T struct{}\n\nfunc (t *T) M() int { return 1 }\n\nfunc F() int { return 2 }\n",
            selector: "src/t.go#T.M",
            outside: Some(("return 2", "return 3")),
            inside: ("return 1", "return 4"),
        },
        Kind {
            path: "src/v.vue",
            content: "<template><div/></template>\n<script setup lang=\"ts\">\nfunction f(): number { return 1 }\nfunction g() { return 2 }\n</script>\n",
            selector: "src/v.vue#f",
            outside: Some(("<div/>", "<span/>")),
            inside: ("return 1", "return 3"),
        },
    ];
    let repo = Repo::new();
    let mut doc = String::new();
    for kind in &kinds {
        repo.write(kind.path, kind.content);
        doc.push_str(&format!("Claim about {}.\n", kind.path));
    }
    repo.write("dir/x.txt", "x\n");
    doc.push_str("Directory claim.\n");
    repo.write("doc.md", &doc);
    repo.git(&["add", "-A"]);
    for (i, kind) in kinds.iter().enumerate() {
        repo.apply("doc.md", i + 1, kind.selector, &[]);
    }
    repo.apply("doc.md", kinds.len() + 1, "dir", &["--ref"]);
    repo.commit("kinds");
    assert_eq!(repo.check(&[]).code, 0, "{}", repo.check(&[]).stdout);

    for kind in &kinds {
        if let Some((from, to)) = kind.outside {
            repo.write(kind.path, &kind.content.replacen(from, to, 1));
            let run = repo.check(&["--worktree"]);
            assert_eq!(run.code, 0, "outside edit of {}: {}", kind.path, run.stdout);
        }
        repo.write(
            kind.path,
            &kind.content.replacen(kind.inside.0, kind.inside.1, 1),
        );
        let run = repo.check(&["--worktree"]);
        assert_eq!(
            run.lines(),
            [format!("doc.md → {}: {STALE}", kind.selector)],
            "inside edit of {}",
            kind.path
        );
        repo.write(kind.path, kind.content);
    }

    std::fs::remove_dir_all(repo.path().join("dir")).unwrap();
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [format!("doc.md → dir: {UNRESOLVED}")]
    );
}

#[test]
fn same_named_members_need_an_explicit_group() {
    let repo = Repo::new();
    let source = "class H {\n  bool Verify(string s) => true;\n  bool Verify(int i) => false;\n}\n";
    repo.write("h.cs", source);
    repo.write(
        "o.ts",
        "export function f(a: string): void;\nexport function f(a: any) {}\n",
    );
    repo.write("doc.md", "Verify claim.\n\nf claim.\n\nHand-written <!-- dinyd: h.cs#H.Verify s1:0123456789abcdef -->\n");
    repo.git(&["add", "-A"]);

    for selector in ["h.cs#H.Verify", "o.ts#f"] {
        let run = repo.dinyd(&["apply", "doc.md", "--line", "1", "--selector", selector]);
        assert_eq!(run.code, 1);
        assert!(
            run.stderr.contains("unresolved in --index"),
            "{}",
            run.stderr
        );
    }
    repo.apply("doc.md", 1, "h.cs#H.Verify(*)", &[]);
    repo.apply("doc.md", 3, "o.ts#f(*)", &[]);
    repo.git(&["add", "-A"]);
    assert_eq!(
        repo.check(&[]).lines(),
        [format!("doc.md → h.cs#H.Verify: {UNRESOLVED}")]
    );

    repo.write("h.cs", &source.replace("=> false", "=> true"));
    let run = repo.check(&["--worktree"]);
    assert_eq!(
        run.lines(),
        [
            format!("doc.md → h.cs#H.Verify(*): {STALE}"),
            format!("doc.md → h.cs#H.Verify: {UNRESOLVED}")
        ]
    );
}

#[test]
fn s1_ignores_whitespace_and_b1_does_not() {
    let repo = Repo::new();
    let source = "export function f(a: number) {\n  // doubles\n  return a * 2;\n}\n";
    repo.write("f.ts", source);
    repo.write("doc.md", "Structural.\n\nBytes.\n");
    repo.git(&["add", "-A"]);
    assert!(repo.apply("doc.md", 1, "f.ts#f", &[]).contains(" s1:"));
    assert!(
        repo.apply("doc.md", 3, "f.ts#f", &["--bytes"])
            .contains(" b1:")
    );
    repo.git(&["add", "-A"]);
    assert_eq!(repo.check(&[]).code, 0);

    repo.write("f.ts", &source.replace("\n", "\r\n"));
    assert_eq!(repo.check(&["--worktree"]).code, 0, "CRLF -> LF keeps both");

    repo.write(
        "f.ts",
        "export function f( a: number )\n{\n    // doubles\n    return a*2;\n}\n",
    );
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [format!("doc.md → f.ts#f: {STALE}")]
    );
    assert!(
        repo.check(&["--worktree"]).stdout.lines().count() == 1,
        "only b1 goes stale"
    );

    for changed in [
        source.replace("doubles", "twice"),
        source.replace("* 2", "* 3"),
    ] {
        repo.write("f.ts", &changed);
        assert_eq!(
            repo.check(&["--worktree"]).lines().len(),
            2,
            "comment and token changes reach s1: {changed}"
        );
    }
}

#[test]
fn apply_refuses_without_writing() {
    let repo = Repo::new();
    repo.write("src/a.cs", CLASS);
    let doc = "# Doc\n\n```\ncode line\n```\n\nClaim.\n";
    let diagram = "---\ntitle: d\n---\nflowchart LR\n  A --> B\n";
    repo.write("doc.md", doc);
    repo.write("d.mermaid", diagram);
    repo.git(&["add", "-A"]);

    let refusals: [(&[&str], &str); 4] = [
        (
            &["doc.md", "--line", "7", "--selector", "src/a.cs#A.Missing"],
            "unresolved in --index",
        ),
        (
            &[
                "doc.md",
                "--line",
                "7",
                "--selector",
                "src/new.cs",
                "--worktree",
            ],
            "unresolved in --worktree",
        ),
        (
            &["doc.md", "--line", "4", "--selector", "src/a.cs#A.F"],
            "would not be read",
        ),
        (
            &["d.mermaid", "--line", "1", "--selector", "src/a.cs#A.F"],
            "would not be read",
        ),
    ];
    for (args, message) in refusals {
        let mut all = vec!["apply"];
        all.extend(args);
        let run = repo.dinyd(&all);
        assert_eq!(run.code, 1, "{args:?}");
        assert!(run.stderr.contains(message), "{args:?}: {}", run.stderr);
    }

    repo.apply("doc.md", 7, "src/a.cs#A.F", &[]);
    repo.apply("d.mermaid", 4, "src/a.cs#A.G", &[]);
    let (doc_now, diagram_now) = (repo.read("doc.md"), repo.read("d.mermaid"));
    for args in [
        ["doc.md", "7", "src/a.cs#A.F"],
        ["d.mermaid", "4", "src/a.cs#A.G"],
    ] {
        let run = repo.dinyd(&["apply", args[0], "--line", args[1], "--selector", args[2]]);
        assert_eq!(run.code, 1);
        assert!(run.stderr.contains("already exists"), "{}", run.stderr);
    }
    let run = repo.dinyd(&[
        "apply",
        "doc.md",
        "--line",
        "7",
        "--selector",
        "src/a.cs",
        "--ref",
        "--bytes",
    ]);
    assert_eq!(run.code, 2);
    assert_eq!(
        (repo.read("doc.md"), repo.read("d.mermaid")),
        (doc_now, diagram_now)
    );
}

#[test]
fn update_rewrites_only_the_digest_in_a_crlf_korean_doc() {
    let repo = Repo::new();
    repo.write("src/a.cs", CLASS);
    let doc = "# 제목\r\n\r\nF는 아무것도 하지 않는다. <!-- dinyd: src/a.cs#A.F s1:0000000000000000 -->\r\n\r\n손으로 쓴 참조 <!-- dinyd: src/a.cs#A.G -->\r\n위임 <!-- dinyd-ref: src -->\r\n끝\r\n";
    repo.write("doc.md", doc);
    let diagram = "flowchart LR\r\n  A --> B\r\n";
    repo.write("d.mermaid", diagram);
    repo.git(&["add", "-A"]);
    assert_eq!(
        repo.check(&[]).lines(),
        [
            format!("doc.md → src/a.cs#A.F: {STALE}"),
            format!("doc.md → src/a.cs#A.G: {UNDECLARED}")
        ]
    );

    let update = |line: &str, selector: &str| {
        repo.dinyd(&[
            "update",
            "doc.md",
            "--line",
            line,
            "--expect-selector",
            selector,
        ])
    };
    let run = update("3", "src/a.cs#A.F");
    assert_eq!(run.code, 0, "{}", run.stderr);
    let digest = run.stdout.trim().to_string();
    let run = update("5", "src/a.cs#A.G");
    assert_eq!(run.code, 0, "{}", run.stderr);
    let filled = run.stdout.trim().to_string();
    let expected = doc
        .replace("s1:0000000000000000", &digest)
        .replace("src/a.cs#A.G -->", &format!("src/a.cs#A.G {filled} -->"));
    assert_eq!(repo.read("doc.md"), expected);
    assert_eq!(update("3", "src/a.cs#A.F").stdout.trim(), "unchanged");

    let run = update("6", "src");
    assert_eq!(run.code, 1);
    assert!(
        run.stderr.contains("delegation reference"),
        "{}",
        run.stderr
    );
    let run = update("3", "src/a.cs#A.G");
    assert_eq!(run.code, 1);
    assert_eq!(repo.read("doc.md"), expected);

    repo.apply("d.mermaid", 1, "src/a.cs#A.F", &[]);
    assert_eq!(
        repo.read("d.mermaid"),
        format!("flowchart LR\r\n%% dinyd: src/a.cs#A.F {digest}\r\n  A --> B\r\n")
    );
    repo.git(&["add", "-A"]);
    assert_eq!(repo.check(&[]).code, 0);
}

#[test]
fn changed_scope_follows_deletions_and_renames() {
    let repo = Repo::new();
    for file in ["src/a.cs", "src/gone.cs", "src/old.cs", "src/b.cs"] {
        repo.write(file, CLASS);
    }
    repo.write("d1.md", "a\n");
    repo.write("d2.md", "gone\n");
    repo.write("d3.md", "old\n");
    repo.write("d4.md", "bad <!-- dinyd-ref: a b.cs -->\n");
    repo.write("d5.md", "b\n\ntext\n");
    repo.git(&["add", "-A"]);
    for (doc, target) in [
        ("d1.md", "src/a.cs"),
        ("d2.md", "src/gone.cs"),
        ("d3.md", "src/old.cs"),
        ("d5.md", "src/b.cs"),
    ] {
        repo.apply(doc, 1, target, &[]);
    }
    repo.write("src/b.cs", &CLASS.replace("F()", "H()"));
    repo.commit("base");

    repo.git(&["rm", "-q", "src/gone.cs"]);
    repo.git(&["mv", "src/old.cs", "src/new.cs"]);
    let changed = vec![
        format!("d2.md → src/gone.cs: {UNRESOLVED}"),
        format!("d3.md → src/old.cs: {UNRESOLVED}"),
        format!("d4.md → a b.cs: {MALFORMED}"),
    ];
    assert_eq!(repo.check(&["--changed"]).lines(), changed);
    assert_eq!(
        repo.check(&[]).lines().len(),
        4,
        "the full scope also reports d5"
    );

    repo.write("d5.md", &repo.read("d5.md").replace("text", "edited"));
    repo.git(&["add", "d5.md"]);
    assert_eq!(
        repo.check(&["--changed"]).lines().len(),
        4,
        "a changed document brings its anchors"
    );

    repo.commit("next");
    assert_eq!(
        repo.check(&["--changed"]).lines(),
        [format!("d4.md → a b.cs: {MALFORMED}")]
    );
    assert_eq!(repo.check(&["--rev", "HEAD", "--changed"]).code, 2);
    assert_eq!(
        repo.check(&["--rev", "HEAD", "--changed", "--base", "HEAD~1"])
            .lines()
            .len(),
        4
    );
    assert_eq!(
        repo.check(&["--changed", "--base", "HEAD~1"]).lines().len(),
        4
    );

    repo.write("src/a.cs", &CLASS.replace("G()", "K()"));
    let run = repo.check(&["--worktree", "--changed"]);
    assert_eq!(
        run.lines(),
        [
            format!("d1.md → src/a.cs: {STALE}"),
            format!("d4.md → a b.cs: {MALFORMED}")
        ]
    );
}

#[test]
fn unmerged_and_intent_to_add_targets_are_unresolved_in_the_index() {
    let repo = Repo::new();
    repo.write("src/a.cs", CLASS);
    repo.write("doc.md", "claim\n\nnew\n");
    repo.git(&["add", "-A"]);
    repo.apply("doc.md", 1, "src/a.cs", &["--bytes"]);
    repo.commit("base");
    let main = repo
        .git(&["rev-parse", "--abbrev-ref", "HEAD"])
        .trim()
        .to_string();

    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.write("src/a.cs", &CLASS.replace("F()", "Side()"));
    repo.commit("side");
    repo.git(&["checkout", "-q", &main]);
    repo.write("src/a.cs", &CLASS.replace("F()", "Main()"));
    repo.commit("main");
    repo.git_output(&["merge", "-q", "side"]);
    assert!(
        !repo.git(&["ls-files", "-u"]).is_empty(),
        "the merge conflicts"
    );

    let expected = [format!("doc.md → src/a.cs: {UNRESOLVED}")];
    assert_eq!(repo.check(&["--index"]).lines(), expected);
    assert_eq!(repo.check(&["--worktree"]).lines(), expected);
    repo.git(&["merge", "--abort"]);

    repo.write("src/n.cs", CLASS);
    repo.git(&["add", "-N", "src/n.cs"]);
    repo.apply("doc.md", 3, "src/n.cs", &["--worktree"]);
    repo.git(&["add", "doc.md"]);
    // src/a.cs differs from the anchored base on every basis; only the index rejects src/n.cs.
    let stale = format!("doc.md → src/a.cs: {STALE}");
    assert_eq!(
        repo.check(&["--index"]).lines(),
        [stale.clone(), format!("doc.md → src/n.cs: {UNRESOLVED}")]
    );
    assert_eq!(repo.check(&["--worktree"]).lines(), [stale]);
}

#[test]
fn s1_keeps_whitespace_inside_strings_and_scalars() {
    let repo = Repo::new();
    let yaml = "sep: \" \"\nother: 1\n";
    let script = "export const label = \"a b\";\n";
    repo.write("cfg.yaml", yaml);
    repo.write("l.ts", script);
    repo.write("doc.md", "Separator.\n\nWhole file.\n\nLabel.\n");
    repo.git(&["add", "-A"]);
    repo.apply("doc.md", 1, "cfg.yaml#/sep", &[]);
    repo.apply("doc.md", 3, "cfg.yaml", &[]);
    repo.apply("doc.md", 5, "l.ts#label", &[]);
    repo.git(&["add", "-A"]);
    assert_eq!(repo.check(&[]).code, 0);

    repo.write("cfg.yaml", &yaml.replace("\" \"", "\"  \""));
    repo.write("l.ts", &script.replace("a b", "a  b"));
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [
            format!("doc.md → cfg.yaml#/sep: {STALE}"),
            format!("doc.md → cfg.yaml: {STALE}"),
            format!("doc.md → l.ts#label: {STALE}"),
        ]
    );
}

#[test]
fn removing_export_changes_the_declaration_digest() {
    let repo = Repo::new();
    let source = "export function f() { return 1; }\nexport const k = 2;\n";
    repo.write("m.ts", source);
    repo.write("doc.md", "f is exported.\n\nk is exported.\n");
    repo.git(&["add", "-A"]);
    repo.apply("doc.md", 1, "m.ts#f", &[]);
    repo.apply("doc.md", 3, "m.ts#k", &[]);
    repo.git(&["add", "-A"]);
    assert_eq!(repo.check(&[]).code, 0);

    repo.write("m.ts", &source.replace("export ", ""));
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [
            format!("doc.md → m.ts#f: {STALE}"),
            format!("doc.md → m.ts#k: {STALE}")
        ]
    );
}

#[test]
fn css_strings_are_exact_in_rule_selectors() {
    let repo = Repo::new();
    let css = "[aria-label=\"Save, close\"] > .icon { color: red; }\n";
    repo.write("site.css", css);
    repo.write(
        "doc.md",
        "Save button <!-- dinyd-ref: site.css#[aria-label=\"Save, close\"]>.icon -->\n",
    );
    repo.git(&["add", "-A"]);
    assert_eq!(repo.check(&[]).code, 0, "{}", repo.check(&[]).stderr);

    repo.write("site.css", &css.replace("Save, close", "Save,close"));
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [format!(
            "doc.md → site.css#[aria-label=\"Save, close\"]>.icon: {UNRESOLVED}"
        )]
    );
}
