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

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
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
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
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

const STALE: &str = "stale — re-judge the claim, then run digest";
const UNRESOLVED: &str = "unresolved — re-judge the claim and the reference";
const MALFORMED: &str = "malformed — re-judge the claim and the reference";

const CLASS: &str = "class A {\n  void F() { }\n}\n";

#[test]
fn s1_keeps_whitespace_inside_yaml_scalars() {
    let repo = Repo::new();
    let yaml = "sep: \" \"\nother: 1\n";
    repo.write("cfg.yaml", yaml);
    repo.write("doc.md", "Separator.\n");
    repo.git(&["add", "-A"]);
    repo.apply("doc.md", 1, "cfg.yaml#/sep", &[]);
    repo.git(&["add", "-A"]);
    assert_eq!(repo.check(&[]).code, 0);

    repo.write("cfg.yaml", &yaml.replace("\" \"", "\"  \""));
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [format!("doc.md → cfg.yaml#/sep: {STALE}")]
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
    repo.write("doc.md", "Save button\n");
    repo.git(&["add", "-A"]);
    repo.apply(
        "doc.md",
        1,
        "site.css#[aria-label=\"Save, close\"]>.icon",
        &[],
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
fn vue_reads_spaced_script_attributes_and_skips_commented_scripts() {
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
fn worktree_rejects_gitlinks_like_the_index() {
    let repo = Repo::new();
    repo.write("vendor/lib/file.txt", "x\n");
    repo.write("doc.md", "[Vendored](vendor/lib)\n");
    repo.git(&["add", "doc.md"]);
    let head = "0123456789012345678901234567890123456789";
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{head},vendor/lib"),
    ]);
    let expected = [format!("doc.md → link vendor/lib: {UNRESOLVED}")];
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
fn broken_anchors_stay_on_one_line_without_digests() {
    let repo = Repo::new();
    repo.write("a.ts", "export function f() {}\n");
    repo.write("inline.md", "Claim. <!-- dinyd: a.ts s1:0123456789abcdef");
    repo.write(
        "block.md",
        "<!-- dinyd: a.ts#f s1:fedcba9876543210\nThis claim must stay in the document.\n",
    );
    repo.write(
        "mode.md",
        "Mode <!-- dinyd: a.ts#f s2:0123456789abcdef -->\n",
    );
    repo.git(&["add", "-A"]);
    assert_eq!(
        repo.check(&[]).lines(),
        [
            format!("block.md → a.ts#f: {MALFORMED}"),
            format!("inline.md → a.ts: {MALFORMED}"),
            format!("mode.md → a.ts#f: {MALFORMED}"),
        ]
    );
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
    // A failing later anchor shows that the lost terminator did not swallow it.
    std::fs::remove_file(repo.path().join("b.ts")).unwrap();
    repo.git(&["add", "-A"]);
    assert_eq!(
        repo.check(&[]).lines(),
        [
            format!("doc.md → a.ts: {MALFORMED}"),
            format!("doc.md → b.ts: {UNRESOLVED}"),
            format!("multi.md → a.ts: {MALFORMED}"),
        ]
    );
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
fn rust_symbols_nest_under_types_traits_and_modules() {
    let repo = Repo::new();
    let source = "#[derive(Clone)]\nstruct S;\nimpl S { fn f(&self) {} }\nimpl std::fmt::Display for S {\n    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) }\n}\ntrait T { fn m(&self); }\ntrait Label { fn label(&self); }\nimpl Label for &str { fn label(&self) {} }\nmacro_rules! m { () => {}; }\nmod a {\n    struct S2 {\n        #[serde(rename = \"wire\")]\n        name: String,\n    }\n    enum E { A, B }\n}\n";
    repo.write("lib.rs", source);
    let selectors = [
        "lib.rs#S",
        "lib.rs#S.f",
        "lib.rs#S.Display",
        "lib.rs#S.Display.fmt",
        "lib.rs#T.m",
        "lib.rs#str.Label.label",
        "lib.rs#m",
        "lib.rs#a.S2.name",
        "lib.rs#a.E.A",
    ];
    repo.write("doc.md", &"claim\n".repeat(selectors.len()));
    repo.git(&["add", "-A"]);
    for (i, selector) in selectors.iter().enumerate() {
        repo.apply("doc.md", i + 1, selector, &[]);
    }
    repo.git(&["add", "-A"]);
    assert_eq!(repo.check(&[]).code, 0);

    // Leading attributes belong to the item they precede.
    repo.write(
        "lib.rs",
        &source
            .replace("derive(Clone)", "derive(Clone, Copy)")
            .replace("\"wire\"", "\"name\""),
    );
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [
            format!("doc.md → lib.rs#S: {STALE}"),
            format!("doc.md → lib.rs#a.S2.name: {STALE}"),
        ]
    );
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
    repo.write("notes.md", "[Gone](missing.cs)\n");
    repo.write("doc.md", "F claim.\n");
    repo.git(&["add", "doc.md"]);
    repo.apply("doc.md", 1, "link.cs#A.F", &["--worktree"]);
    assert_eq!(
        repo.check(&["--worktree"]).lines(),
        [format!("notes.md → link missing.cs: {UNRESOLVED}")]
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
fn comments_and_frontmatter_hold_no_links() {
    let repo = Repo::new();
    repo.write(
        "doc.md",
        "<!--\n@import \"m1.mermaid\"\n-->\n\n\
         Disabled: <!--\n@import \"m2.mermaid\"\n-->\n",
    );
    repo.write(
        "front.md",
        "---\ndescription: |\n  @import \"m3.mermaid\"\n  [front](m4.md)\n---\n\nbody\n",
    );
    repo.commit("base");
    let run = repo.check(&[]);
    assert_eq!((run.code, run.stdout.as_str()), (0, ""), "{}", run.stderr);
}

#[test]
fn a_link_naming_no_path_is_selected_only_with_its_document() {
    let repo = Repo::new();
    repo.write("a.md", "a\n");
    repo.commit("base");
    repo.write("d.md", "[out](../x.md)\n");
    repo.commit("out");
    repo.write("unrelated.txt", "x\n");
    repo.git(&["add", "unrelated.txt"]);
    assert_eq!(repo.check(&["--changed"]).lines(), Vec::<String>::new());
    assert_eq!(
        repo.check(&["--changed", "--base", "HEAD~1"]).lines(),
        [format!("d.md → link ../x.md: {UNRESOLVED}")]
    );
}

#[test]
fn a_broken_destination_is_escaped_in_both_outputs() {
    let repo = Repo::new();
    repo.write("doc.md", "[tab](a&#9;b.md)\n");
    repo.commit("base");
    let run = repo.check(&[]);
    assert_eq!(
        run.lines(),
        [format!("doc.md → link a\\tb.md: {UNRESOLVED}")]
    );
    assert!(
        run.stderr.contains("no such path: a\\tb.md\n"),
        "{}",
        run.stderr
    );
}
