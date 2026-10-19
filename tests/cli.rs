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

#[test]
fn vue_reads_only_top_level_script_elements() {
    let repo = Repo::new();
    let live = "<template><div/></template>\n<script setup lang = \"ts\">\nfunction f(n: number) { return n }\n</script>\n";
    repo.write("v.vue", live);
    repo.write("doc.md", "f claim.\n");
    repo.git(&["add", "-A"]);
    repo.apply("doc.md", 1, "v.vue#f", &[]);
    repo.git(&["add", "-A"]);
    assert_eq!(repo.check(&[]).code, 0);

    let commented = live
        .replace("<script", "<!-- <script")
        .replace("</script>", "</script> -->");
    repo.write("v.vue", &commented);
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [format!("doc.md → v.vue#f: {UNRESOLVED}")]
    );
}

#[test]
fn broken_anchors_fail_without_echoing_digests() {
    let repo = Repo::new();
    repo.write("a.ts", "export function f() {}\n");
    repo.write(
        "doc.md",
        "Mode <!-- dinyd: a.ts#f s2:0123456789abcdef -->\n\n<!-- dinyd: a.ts s1:fedcba9876543210",
    );
    repo.git(&["add", "-A"]);
    let run = repo.check(&[]);
    assert_eq!(
        run.lines(),
        [
            format!("doc.md → a.ts#f: {MALFORMED}"),
            format!("doc.md → a.ts: {MALFORMED}"),
        ]
    );
    assert!(!run.stdout.contains("0123456789abcdef") && !run.stdout.contains("fedcba9876543210"));
}

#[test]
fn worktree_rejects_gitlinks_like_the_index() {
    let repo = Repo::new();
    repo.write("vendor/lib/file.txt", "x\n");
    repo.write("doc.md", "Vendored <!-- dinyd-ref: vendor/lib -->\n");
    repo.git(&["add", "doc.md"]);
    let head = "0123456789012345678901234567890123456789";
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{head},vendor/lib"),
    ]);
    let expected = [format!("doc.md → vendor/lib: {UNRESOLVED}")];
    assert_eq!(repo.check(&["--index"]).lines(), expected);
    assert_eq!(repo.check(&["--worktree"]).lines(), expected);
}

#[test]
fn b1_hashes_exactly_the_selected_bytes_and_apply_names_its_basis() {
    let repo = Repo::new();
    repo.write("cfg.json", "{\"a\":1}\n");
    repo.write("doc.md", "a is one.\n");
    repo.git(&["add", "-A"]);
    let run = repo.dinyd(&[
        "apply",
        "doc.md",
        "--line",
        "1",
        "--selector",
        "cfg.json#/a",
        "--bytes",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    // SHA-256("1") starts with 6b86b273ff34fce1.
    assert_eq!(run.stdout.trim(), "dinyd: cfg.json#/a b1:6b86b273ff34fce1");
    assert!(run.stderr.contains("--index"), "{}", run.stderr);
}

#[test]
fn s1_treats_code_inside_template_strings_as_code() {
    let repo = Repo::new();
    let source = "function f(a) { return `n=${a+1}`; }\n";
    repo.write("f.js", source);
    repo.write("doc.md", "f formats n.\n");
    repo.git(&["add", "-A"]);
    repo.apply("doc.md", 1, "f.js#f", &[]);
    repo.git(&["add", "-A"]);

    repo.write("f.js", &source.replace("a+1", "a + 1"));
    assert_eq!(
        repo.check(&["--worktree"]).code,
        0,
        "spacing inside ${{}} is formatting"
    );
    for changed in [source.replace("n=", "n= "), source.replace("a+1", "a+2")] {
        repo.write("f.js", &changed);
        assert_eq!(
            repo.check(&["--worktree"]).lines(),
            [format!("doc.md → f.js#f: {STALE}")],
            "{changed}"
        );
    }
}

#[test]
fn unterminated_anchors_stay_on_their_own_line() {
    let repo = Repo::new();
    repo.write("a.ts", "export function f() {}\n");
    repo.write("inline.md", "Claim. <!-- dinyd: a.ts s1:0123456789abcdef");
    repo.write(
        "block.md",
        "<!-- dinyd: a.ts#f s1:fedcba9876543210\nThis claim must stay in the document.\n",
    );
    repo.git(&["add", "-A"]);
    let run = repo.check(&[]);
    assert_eq!(
        run.lines(),
        [
            format!("block.md → a.ts#f: {MALFORMED}"),
            format!("inline.md → a.ts: {MALFORMED}"),
        ]
    );
    assert!(!run.stdout.contains("0123456789abcdef") && !run.stdout.contains("fedcba9876543210"));
    assert!(!run.stdout.contains("Claim") && !run.stdout.contains("must stay"));
}

#[test]
fn a_lost_terminator_before_another_anchor_and_escaped_examples() {
    let repo = Repo::new();
    repo.write("a.ts", "export function f() {}\n");
    repo.write("b.ts", "export function g() {}\n");
    repo.write("doc.md", "Claim B.\n");
    repo.git(&["add", "-A"]);
    let b = repo.apply("doc.md", 1, "b.ts", &[]);
    repo.write(
        "doc.md",
        &format!(
            "<!-- dinyd: a.ts s1:0123456789abcdef\nClaim B. <!-- {b} -->\n\nThe notation is \\<!-- dinyd: a.ts -->.\n"
        ),
    );
    repo.write(
        "multi.md",
        "<!-- dinyd: a.ts s1:0123456789abcdef\nClaim C.\n-->\n",
    );
    repo.git(&["add", "-A"]);
    let run = repo.check(&[]);
    assert_eq!(
        run.lines(),
        [
            format!("doc.md → a.ts: {MALFORMED}"),
            format!("multi.md → a.ts: {MALFORMED}"),
        ]
    );
    assert!(!run.stdout.contains("0123456789abcdef") && !run.stdout.contains("Claim"));
}

#[test]
fn nested_and_conditional_declarations_are_symbols() {
    let repo = Repo::new();
    repo.write(
        "m.ts",
        "export enum E { A = 1, B = 2 }\nnamespace Foo.Bar { export class Thing { run(): number { return 1; } } }\nmodule M { export function b() { return 1; } }\ndeclare module \"fs\" { export function readFile(path: string): void; }\ndeclare module \"decimal.js\" { export function config(): void; }\n",
    );
    repo.write(
        "a.cs",
        "#if DEBUG\nclass A { void F() { } }\n#endif\nclass B { void G() { } }\n",
    );
    let selectors = [
        "m.ts#E.A",
        "m.ts#Foo.Bar.Thing.run",
        "m.ts#M.b",
        "m.ts#fs.readFile",
        "m.ts#decimal.js.config",
        "a.cs#A.F",
    ];
    repo.write("doc.md", &"claim\n".repeat(selectors.len()));
    repo.git(&["add", "-A"]);
    for (i, selector) in selectors.iter().enumerate() {
        repo.apply("doc.md", i + 1, selector, &[]);
    }
    repo.git(&["add", "-A"]);
    assert_eq!(repo.check(&[]).code, 0);
}

#[test]
fn worktree_judges_the_file_type_on_disk() {
    let repo = Repo::new();
    // The index records link.cs and notes.md as symlinks; on disk they are now regular files.
    let blob = |content: &str| {
        repo.write("blob.tmp", content);
        let oid = repo
            .git(&["hash-object", "-w", "blob.tmp"])
            .trim()
            .to_string();
        std::fs::remove_file(repo.path().join("blob.tmp")).unwrap();
        oid
    };
    let link = blob("real.cs");
    for path in ["link.cs", "notes.md"] {
        repo.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("120000,{link},{path}"),
        ]);
    }
    repo.write("link.cs", CLASS);
    repo.write("notes.md", "Gone <!-- dinyd-ref: missing.cs -->\n");
    repo.write("doc.md", "F claim.\n");
    repo.git(&["add", "doc.md"]);
    repo.apply("doc.md", 1, "link.cs#A.F", &["--worktree"]);
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [format!("notes.md → missing.cs: {UNRESOLVED}")]
    );
}

#[test]
fn an_anchor_after_a_table_row_is_read() {
    let repo = Repo::new();
    repo.write("a.cs", CLASS);
    repo.write("doc.md", "| a | b |\n| --- | --- |\n| F | does nothing |\n");
    repo.git(&["add", "-A"]);
    repo.apply("doc.md", 3, "a.cs#A.F", &[]);
    repo.git(&["add", "-A"]);
    assert_eq!(repo.check(&[]).code, 0);
    repo.write(
        "a.cs",
        &CLASS.replace("void F() { }", "void F() { return; }"),
    );
    repo.git(&["add", "-A"]);
    assert_eq!(
        repo.check(&[]).lines(),
        [format!("doc.md → a.cs#A.F: {STALE}")]
    );
}

#[test]
fn a_document_unit_tracks_only_its_wrapped_span() {
    let repo = Repo::new();
    let spec = "Alpha holds. <!-- dinyd-id: beta -->beta.<!-- /dinyd-id: beta --> Gamma holds.\n\nOther paragraph.\n";
    repo.write("spec.md", spec);
    repo.write("doc.md", "Beta is restated here.\n");
    repo.git(&["add", "-A"]);
    // SHA-256("beta.") starts with 49a242e6143002e5.
    assert_eq!(
        repo.apply("doc.md", 1, "spec.md#beta", &[]),
        "dinyd: spec.md#beta d1:49a242e6143002e5"
    );
    let ok = repo.check(&["--worktree"]);
    assert_eq!((ok.code, ok.stdout.as_str()), (0, ""));

    repo.write(
        "spec.md",
        &spec
            .replace("Gamma holds.", "Gamma holds!")
            .replace("Other", "Another"),
    );
    assert_eq!(repo.check(&["--worktree"]).code, 0);

    repo.write("spec.md", &spec.replace("beta.<", "beta!<"));
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [format!("doc.md → spec.md#beta: {STALE}")]
    );
    let run = repo.dinyd(&[
        "update",
        "doc.md",
        "--line",
        "1",
        "--expect-selector",
        "spec.md#beta",
        "--worktree",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stdout.starts_with("d1:"), "{}", run.stdout);
    assert_eq!(repo.check(&["--worktree"]).code, 0);
}

#[test]
fn dinyd_comments_inside_a_unit_are_projected_out() {
    let repo = Repo::new();
    repo.write("src/a.cs", CLASS);
    let spec = "<!-- dinyd-id: outer -->\nalpha <!-- dinyd-id: w -->beta<!-- /dinyd-id: w -->\n<!-- /dinyd-id: outer -->\n";
    repo.write("spec.md", spec);
    repo.write("doc.md", "Outer.\n\nInner.\n");
    repo.git(&["add", "-A"]);
    // Own-line boundaries drop their lines; inline markers are zero-width. SHA-256 of
    // "alpha beta\n" starts with 87de0dca21b24293.
    assert_eq!(
        repo.apply("doc.md", 1, "spec.md#outer", &[]),
        "dinyd: spec.md#outer d1:87de0dca21b24293"
    );
    repo.apply("doc.md", 3, "spec.md#w", &[]);
    assert_eq!(repo.check(&["--worktree"]).code, 0);

    // The target's own anchor, written by apply and later updated, never reaches the digest.
    repo.apply("spec.md", 2, "src/a.cs#A.F", &["--worktree"]);
    assert_eq!(repo.check(&["--worktree"]).code, 0);
    repo.write(
        "src/a.cs",
        &CLASS.replace("void F() { }", "void F() { return; }"),
    );
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [format!("spec.md → src/a.cs#A.F: {STALE}")]
    );
    let run = repo.dinyd(&[
        "update",
        "spec.md",
        "--line",
        "2",
        "--expect-selector",
        "src/a.cs#A.F",
        "--worktree",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(repo.check(&["--worktree"]).code, 0);

    // Removing the inner unit's markers leaves the outer unit's bytes as they were.
    let unwrapped = repo
        .read("spec.md")
        .replace("<!-- dinyd-id: w -->", "")
        .replace("<!-- /dinyd-id: w -->", "");
    repo.write("spec.md", &unwrapped);
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [format!("doc.md → spec.md#w: {UNRESOLVED}")]
    );

    repo.write("spec.md", &unwrapped.replace("beta", "BETA"));
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [
            format!("doc.md → spec.md#outer: {STALE}"),
            format!("doc.md → spec.md#w: {UNRESOLVED}"),
        ]
    );
}

#[test]
fn boundaries_are_own_line_or_inline_each_on_its_own() {
    let repo = Repo::new();
    repo.write(
        "spec.md",
        "- item\r\n  <!-- dinyd-id: own -->\r\nline one\r\n  <!-- /dinyd-id: own -->\r\n\r\npre <!-- dinyd-id: inline --> mid <!-- /dinyd-id: inline --> post\r\n",
    );
    repo.write("doc.md", "Own.\n\nInline.\n");
    repo.git(&["add", "-A"]);
    // SHA-256("line one\n") and SHA-256(" mid "): CRLF -> LF, no edge trim.
    assert_eq!(
        repo.apply("doc.md", 1, "spec.md#own", &[]),
        "dinyd: spec.md#own d1:31f21b1dae81d3f3"
    );
    assert_eq!(
        repo.apply("doc.md", 3, "spec.md#inline", &[]),
        "dinyd: spec.md#inline d1:b83fac3baf0d0a46"
    );
}

#[test]
fn pairing_errors_are_unresolved_on_each_referer() {
    let repo = Repo::new();
    repo.write(
        "spec.md",
        "<!-- dinyd-id: lone -->a\n\n<!-- /dinyd-id: back -->b<!-- dinyd-id: back -->\n\n<!-- dinyd-id: dup -->c<!-- /dinyd-id: dup --><!-- dinyd-id: dup -->\n\n<!-- dinyd-id: fine -->d<!-- /dinyd-id: fine -->\n\n<!-- dinyd-id: orphan -->e\n\n```\n<!-- dinyd-id: code -->f<!-- /dinyd-id: code -->\n```\n",
    );
    repo.write(
        "doc.md",
        "L <!-- dinyd: spec.md#lone -->\n\nB <!-- dinyd: spec.md#back -->\n\nD <!-- dinyd-ref: spec.md#dup -->\n\nM <!-- dinyd: spec.md#missing -->\n\nC <!-- dinyd-ref: spec.md#code -->\n\nF <!-- dinyd-ref: spec.md#fine -->\n",
    );
    repo.git(&["add", "-A"]);
    let run = repo.check(&[]);
    assert_eq!(
        run.lines(),
        [
            format!("doc.md → spec.md#lone: {UNRESOLVED}"),
            format!("doc.md → spec.md#back: {UNRESOLVED}"),
            format!("doc.md → spec.md#dup: {UNRESOLVED}"),
            format!("doc.md → spec.md#missing: {UNRESOLVED}"),
            format!("doc.md → spec.md#code: {UNRESOLVED}"),
        ]
    );
    for reason in [
        "unit lone has no end",
        "unit back ends before it starts",
        "unit dup is declared more than once",
        "no unit missing",
        "no unit code",
    ] {
        assert!(run.stderr.contains(reason), "{reason}: {}", run.stderr);
    }

    let before = repo.read("doc.md");
    let refused = repo.dinyd(&[
        "apply",
        "doc.md",
        "--line",
        "1",
        "--selector",
        "spec.md#lone",
    ]);
    assert_eq!(refused.code, 1);
    assert!(
        refused.stderr.contains("unit lone has no end"),
        "{}",
        refused.stderr
    );
    assert_eq!(repo.read("doc.md"), before);
}

#[test]
fn a_declaration_that_does_not_parse_is_malformed() {
    let repo = Repo::new();
    repo.write(
        "spec.md",
        "A <!-- dinyd-id: bad name -->a<!-- /dinyd-id: -->\n\nB <!-- dinyd-id: good -->b<!-- /dinyd-id: good -->\n",
    );
    repo.git(&["add", "-A"]);
    assert_eq!(
        repo.check(&[]).lines(),
        [
            format!("spec.md → dinyd-id: bad name: {MALFORMED}"),
            format!("spec.md → /dinyd-id:: {MALFORMED}"),
        ]
    );
}

#[test]
fn a_unit_takes_only_d1() {
    let repo = Repo::new();
    repo.write("spec.md", "<!-- dinyd-id: u -->u<!-- /dinyd-id: u -->\n");
    repo.write("cfg.json", "{\"a\":1}\n");
    repo.write(
        "doc.md",
        "Claim.\n\nS <!-- dinyd: spec.md#u s1:0123456789abcdef -->\n\nJ <!-- dinyd: cfg.json#/a d1:0123456789abcdef -->\n",
    );
    repo.git(&["add", "-A"]);
    let before = repo.read("doc.md");
    let run = repo.dinyd(&[
        "apply",
        "doc.md",
        "--line",
        "1",
        "--selector",
        "spec.md#u",
        "--bytes",
    ]);
    assert_eq!(run.code, 2, "{}", run.stderr);
    assert_eq!(repo.read("doc.md"), before);

    assert_eq!(
        repo.check(&[]).lines(),
        [
            format!("doc.md → spec.md#u: {UNRESOLVED}"),
            format!("doc.md → cfg.json#/a: {UNRESOLVED}"),
        ]
    );
    let whole = repo.apply("doc.md", 1, "spec.md", &[]);
    assert!(whole.contains(" b1:"), "{whole}");
}

#[test]
fn a_mermaid_unit_is_the_lines_between_its_markers() {
    let repo = Repo::new();
    let diagram = "flowchart LR\n  %% dinyd-id: edge\n  A --> B\n  %% /dinyd-id: edge\n  B --> C\n";
    repo.write("d.mermaid", diagram);
    repo.write("doc.md", "A leads to B.\n");
    repo.git(&["add", "-A"]);
    // SHA-256("  A --> B\n") starts with be52d6887f5770ba.
    assert_eq!(
        repo.apply("doc.md", 1, "d.mermaid#edge", &[]),
        "dinyd: d.mermaid#edge d1:be52d6887f5770ba"
    );
    repo.write("d.mermaid", &diagram.replace("B --> C", "B --> D"));
    assert_eq!(repo.check(&["--worktree"]).code, 0);
    repo.write("d.mermaid", &diagram.replace("A --> B", "A --> X"));
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [format!("doc.md → d.mermaid#edge: {STALE}")]
    );

    repo.write("d.mmd", diagram);
    repo.git(&["add", "-A"]);
    assert_eq!(
        repo.apply("doc.md", 1, "d.mmd#edge", &[]),
        "dinyd: d.mmd#edge d1:be52d6887f5770ba",
        "a .mmd file is a Mermaid document"
    );
}

#[test]
fn a_link_follows_renames_in_each_view() {
    let repo = Repo::new();
    repo.write("docs/verification/checks.md", "checks\n");
    repo.write(
        "docs/guide.md",
        "Read [Checks](verification/checks.md) first.\n",
    );
    repo.commit("base");
    assert_eq!(repo.check(&[]).code, 0);

    repo.git(&[
        "mv",
        "docs/verification/checks.md",
        "docs/verification/rules.md",
    ]);
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.lines(),
        [format!(
            "docs/guide.md → link verification/checks.md: {UNRESOLVED}"
        )]
    );
    assert!(
        run.stderr
            .contains("no such path: docs/verification/checks.md"),
        "{}",
        run.stderr
    );

    repo.git(&[
        "mv",
        "docs/verification/rules.md",
        "docs/verification/checks.md",
    ]);
    std::fs::rename(
        repo.path().join("docs/verification/checks.md"),
        repo.path().join("docs/verification/rules.md"),
    )
    .unwrap();
    assert_eq!(repo.check(&[]).code, 0, "the index still has the target");
    assert_eq!(repo.check(&["--worktree"]).code, 1);

    std::fs::rename(
        repo.path().join("docs/verification/rules.md"),
        repo.path().join("docs/verification/checks.md"),
    )
    .unwrap();
    repo.write(
        "docs/guide.md",
        "Read [Rules](verification/rules.md) first.\n",
    );
    assert_eq!(
        repo.check(&[]).code,
        0,
        "the index still has the old document"
    );
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [format!(
            "docs/guide.md → link verification/rules.md: {UNRESOLVED}"
        )]
    );
    repo.commit("rules");
    assert_eq!(repo.check(&["--rev", "HEAD~1"]).code, 0);
    assert_eq!(repo.check(&["--rev", "HEAD"]).code, 1);
}

#[test]
fn links_resolve_from_the_document_directory() {
    let repo = Repo::new();
    repo.write("top.md", "top\n");
    repo.write("src/main.go", "package main\n");
    repo.write(
        "a/b/doc.md",
        "[up](../../top.md) [dir](../../src/) [bare dir](../../src) [rooted](/src/main.go) [root](/)\n\n\
         [file as dir](../../top.md/) [out](../../../x.md)\n",
    );
    repo.commit("base");
    let run = repo.check(&[]);
    assert_eq!(
        run.lines(),
        [
            format!("a/b/doc.md → link ../../top.md/: {UNRESOLVED}"),
            format!("a/b/doc.md → link ../../../x.md: {UNRESOLVED}"),
        ]
    );
    assert!(
        run.stderr.contains("not a directory: top.md"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("leaves the repository"),
        "{}",
        run.stderr
    );
}

#[test]
fn urls_fragments_and_code_are_not_links() {
    let repo = Repo::new();
    repo.write(
        "doc.md",
        "[url](https://example.com/m1.md) <https://example.com/m2.md> [mail](mailto:a@b.c) \
         [host](//host/m3.md) [frag](#m4) [empty]()\n\n\
         ```md\n[fenced](m5.md)\n@import \"m6.mermaid\"\n```\n\n\
         `[inline](m7.md)` and `tools/m8.mjs`\n\n\
         \x20   [indented](m9.md)\n\n\
         <a href=\"m10.md\">raw</a>\n\n\
         <!--\n@import \"m11.mermaid\"\n-->\n\n\
         Disabled: <!--\n@import \"m15.mermaid\"\n-->\n\n\
         <div>\n@import \"m12.mermaid\"\n</div>\n",
    );
    repo.write(
        "front.md",
        "---\ndescription: |\n  @import \"m13.mermaid\"\n  [front](m14.md)\n---\n\nbody\n",
    );
    repo.commit("base");
    let run = repo.check(&[]);
    assert_eq!((run.code, run.stdout.as_str()), (0, ""), "{}", run.stderr);
}

#[test]
fn link_paths_are_decoded_and_imports_are_literal() {
    let repo = Repo::new();
    repo.write("my notes.md", "notes\n");
    repo.write("a+b.md", "plus\n");
    repo.write("flow.mermaid", "flowchart LR\n  A --> B\n");
    repo.write(
        "doc.md",
        "[q](a+b.md?raw=1#top) [space](my%20notes.md) [angle](<my notes.md>)\n\n\
         @import \"flow.mermaid\"\n\
         @import \"a%2Bb.md\"\n\
         \x20 @import \"gone.mermaid\" {width=\"300px\"}\n",
    );
    repo.commit("base");
    assert_eq!(
        repo.check(&[]).lines(),
        [
            format!("doc.md → link a%2Bb.md: {UNRESOLVED}"),
            format!("doc.md → link gone.mermaid: {UNRESOLVED}"),
        ]
    );
}

#[test]
fn deleting_a_target_selects_the_documents_that_link_it() {
    let repo = Repo::new();
    repo.write("t/target.md", "target\n");
    repo.write("t/other.md", "other\n");
    repo.write("a.md", "[target](t/target.md)\n");
    repo.write("b.md", "[dir](t/)\n");
    repo.write("c.md", "[old](gone.md)\n");
    repo.commit("base");

    repo.git(&["rm", "-q", "t/target.md"]);
    assert_eq!(
        repo.check(&["--changed"]).lines(),
        [format!("a.md → link t/target.md: {UNRESOLVED}")]
    );
    assert_eq!(
        repo.check(&[]).lines().len(),
        2,
        "the full scope also reports c.md"
    );

    repo.git(&["rm", "-q", "t/other.md"]);
    assert_eq!(
        repo.check(&["--changed"]).lines(),
        [
            format!("a.md → link t/target.md: {UNRESOLVED}"),
            format!("b.md → link t/: {UNRESOLVED}"),
        ],
        "removing the last file under a linked directory selects its document"
    );

    repo.commit("next");
    repo.write("d.md", "[out](../x.md)\n");
    repo.commit("out");
    repo.write("unrelated.txt", "x\n");
    repo.git(&["add", "unrelated.txt"]);
    assert_eq!(
        repo.check(&["--changed"]).lines(),
        Vec::<String>::new(),
        "a link naming no path is selected only with its document"
    );
    assert_eq!(
        repo.check(&["--changed", "--base", "HEAD~1"]).lines(),
        [format!("d.md → link ../x.md: {UNRESOLVED}")]
    );
}

#[test]
fn a_broken_destination_is_reported_once_without_prose() {
    let repo = Repo::new();
    repo.write(
        "doc.md",
        "See [the secret label][ref], [again](gone.md), and [twice](gone.md \"Secret Title\").\n\n\
         ![alt prose](img/missing.png) [tab](a&#9;b.md)\n\n\
         [ref]: ref-gone.md\n",
    );
    repo.commit("base");
    let run = repo.check(&[]);
    assert_eq!(
        run.lines(),
        [
            format!("doc.md → link ref-gone.md: {UNRESOLVED}"),
            format!("doc.md → link gone.md: {UNRESOLVED}"),
            format!("doc.md → link img/missing.png: {UNRESOLVED}"),
            format!("doc.md → link a\\tb.md: {UNRESOLVED}"),
        ]
    );
    assert!(
        run.stderr.contains("no such path: a\\tb.md\n"),
        "{}",
        run.stderr
    );
    assert!(!run.stderr.contains('\t'), "{}", run.stderr);
}
