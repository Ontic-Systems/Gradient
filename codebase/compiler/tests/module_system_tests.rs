//! Module System Tests - Import Statements
//!
//! Tests for the import statement parsing (Issue #144).

use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser::parse;
use gradient_compiler::ast::ItemKind;

fn parse_source(source: &str) -> (gradient_compiler::ast::Module, Vec<gradient_compiler::parser::ParseError>) {
    let mut lexer = Lexer::new(source, 0);
    let tokens = lexer.tokenize();
    parse(tokens, 0)
}

/// Test parsing a simple import statement as a top-level item
#[test]
fn parse_simple_import() {
    let source = r#"import "./lexer.gr""#;
    let (module, errors) = parse_source(source);
    
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::Import { path, alias } => {
            assert_eq!(path, "./lexer.gr");
            assert_eq!(*alias, None);
        }
        _ => panic!("Expected Import item, got {:?}", module.items[0].node),
    }
}

/// Test parsing an import with alias as a top-level item
#[test]
fn parse_import_with_alias() {
    let source = r#"import "./parser.gr" as parser_module"#;
    let (module, errors) = parse_source(source);
    
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::Import { path, alias } => {
            assert_eq!(path, "./parser.gr");
            assert_eq!(*alias, Some("parser_module".to_string()));
        }
        _ => panic!("Expected Import item, got {:?}", module.items[0].node),
    }
}

/// Test parsing multiple import statements
#[test]
fn parse_multiple_imports() {
    let source = r#"
import "./lexer.gr"
import "./parser.gr" as parser_module
"#;
    let (module, errors) = parse_source(source);
    
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
    assert_eq!(module.items.len(), 2);
    
    // Check first import
    match &module.items[0].node {
        ItemKind::Import { path, alias } => {
            assert_eq!(path, "./lexer.gr");
            assert_eq!(*alias, None);
        }
        _ => panic!("Expected Import item"),
    }
    
    // Check second import with alias
    match &module.items[1].node {
        ItemKind::Import { path, alias } => {
            assert_eq!(path, "./parser.gr");
            assert_eq!(*alias, Some("parser_module".to_string()));
        }
        _ => panic!("Expected Import item"),
    }
}
