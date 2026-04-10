use gradient_compiler::lexer::Lexer;

fn main() {
    let source = std::fs::read_to_string("/tmp/full_concat_debug.gr").unwrap();
    let mut lexer = Lexer::new(&source, 0);
    let tokens = lexer.tokenize();
    println!("Total tokens: {}", tokens.len());
    
    // Check for error tokens
    let errors: Vec<_> = tokens.iter()
        .filter(|t| matches!(t.kind, gradient_compiler::lexer::token::TokenKind::Error(_)))
        .collect();
    println!("Error tokens: {}", errors.len());
    for (i, t) in errors.iter().take(10).enumerate() {
        println!("  {}: {:?} at {:?}", i, t.kind, t.span);
    }
}
