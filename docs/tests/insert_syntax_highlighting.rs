#[macro_use]
extern crate pretty_assertions;

#[cfg(test)]
mod insert_doc_syntax_highlighting {
    use std::{fs::File, io::Write, path::PathBuf};

    use bumpalo::{collections::String as BumpString, Bump};
    use roc_ast::module::load_module;
    use roc_docs::{syntax_highlight_expr, syntax_highlight_top_level_defs};
    use roc_load::file::LoadedModule;
    use tempfile::tempdir;
    use uuid::Uuid;

    fn expect_html(code_str: &str, want: &str, use_expr: bool) {
        let loaded_module = make_mock_module();

        let code_block_arena = Bump::new();
        let mut code_block_buf = BumpString::new_in(&code_block_arena);

        if use_expr {
            match syntax_highlight_expr(
                &code_block_arena,
                &mut code_block_buf,
                code_str,
                loaded_module.module_id,
                &loaded_module.interns.module_ids,
                &loaded_module.interns,
            ) {
                Ok(highlighted_code_str) => {
                    assert_eq!(highlighted_code_str, want);
                }
                Err(syntax_error) => {
                    panic!("Unexpected parse failure when parsing this for rendering in docs:\n\n{}\n\nParse error was:\n\n{:?}\n\n", code_str, syntax_error)
                }
            };
        } else {
            match syntax_highlight_top_level_defs(
                &code_block_arena,
                &mut code_block_buf,
                code_str,
                loaded_module.module_id,
                &loaded_module.interns.module_ids,
                &loaded_module.interns,
            ) {
                Ok(highlighted_code_str) => {
                    assert_eq!(highlighted_code_str, want);
                }
                Err(syntax_error) => {
                    panic!("Unexpected parse failure when parsing this for rendering in docs:\n\n{}\n\nParse error was:\n\n{:?}\n\n", code_str, syntax_error)
                }
            };
        }
    }

    pub const HELLO_WORLD: &str = r#"
app "test-app"
    packages { base: "platform" }
    imports []
    provides [ main ] to base

main = "Hello, world!"


"#;

    fn make_mock_module() -> LoadedModule {
        let temp_dir = tempdir().expect("Failed to create temporary directory for test.");
        let temp_file_path_buf =
            PathBuf::from([Uuid::new_v4().to_string(), ".roc".to_string()].join(""));
        let temp_file_full_path = temp_dir.path().join(temp_file_path_buf);

        let mut file = File::create(temp_file_full_path.clone()).expect(&format!(
            "Failed to create temporary file for path {:?}",
            temp_file_full_path
        ));
        writeln!(file, "{}", HELLO_WORLD).expect(&format!(
            "Failed to write {:?} to file: {:?}",
            HELLO_WORLD, file
        ));

        load_module(&temp_file_full_path)
    }

    fn expect_html_expr(code_str: &str, want: &str) {
        expect_html(code_str, want, true)
    }

    fn expect_html_def(code_str: &str, want: &str) {
        expect_html(code_str, want, false)
    }

    #[test]
    fn number_expr() {
        expect_html_expr("2", r#"<span class="syntax-number">2</span>"#);
    }

    #[test]
    fn string_expr() {
        expect_html_expr(r#""abc""#, r#"<span class="syntax-string">"abc"</span>"#);
    }

    #[test]
    fn empty_list_expr() {
        expect_html_expr(
            r#"[]"#,
            r#"<span class="syntax-bracket">[ </span><span class="syntax-bracket"> ]</span>"#,
        );
    }

    #[test]
    fn single_elt_list_expr() {
        expect_html_expr(
            r#"[ 0 ]"#,
            r#"<span class="syntax-bracket">[ </span><span class="syntax-number">0</span><span class="syntax-bracket"> ]</span>"#,
        );
    }

    #[test]
    fn multi_elt_list_expr() {
        expect_html_expr(
            r#"[ "hello", "WoRlD" ]"#,
            r#"<span class="syntax-bracket">[ </span><span class="syntax-string">"hello"</span><span class="syntax-comma">, </span><span class="syntax-string">"WoRlD"</span><span class="syntax-bracket"> ]</span>"#,
        );
    }

    #[test]
    fn record_expr() {
        expect_html_expr(
            r#"{ a: "hello!" }"#,
            "<span class=\"syntax-bracket\">{ </span><span class=\"syntax-recordfield\">a</span><span class=\"syntax-operator\">: </span><span class=\"syntax-string\">\"hello!\"</span><span class=\"syntax-bracket\"> }</span>",
        );
    }

    #[test]
    fn nested_record_expr() {
        expect_html_expr(
            r#"{ a: { bB: "WoRlD" } }"#,
            "<span class=\"syntax-bracket\">{ </span><span class=\"syntax-recordfield\">a</span><span class=\"syntax-operator\">: </span><span class=\"syntax-bracket\">{ </span><span class=\"syntax-recordfield\">bB</span><span class=\"syntax-operator\">: </span><span class=\"syntax-string\">\"WoRlD\"</span><span class=\"syntax-bracket\"> }</span><span class=\"syntax-bracket\"> }</span>",
        );
    }

    #[test]
    fn top_level_def_value() {
        expect_html_def(
            r#"main = "Hello, World!""#,
            "<span class=\"syntax-variable\">main</span><span class=\"syntax-operator\"> = </span><span class=\"syntax-string\">\"Hello, World!\"</span>\n\n",
        );
    }

    #[test]
    fn tld_list() {
        expect_html_def(
            r#"main = [ 1, 2, 3 ]"#,
            "<span class=\"syntax-variable\">main</span><span class=\"syntax-operator\"> = </span><span class=\"syntax-bracket\">[ </span><span class=\"syntax-number\">1</span><span class=\"syntax-comma\">, </span><span class=\"syntax-number\">2</span><span class=\"syntax-comma\">, </span><span class=\"syntax-number\">3</span><span class=\"syntax-bracket\"> ]</span>\n\n",
        );
    }
}