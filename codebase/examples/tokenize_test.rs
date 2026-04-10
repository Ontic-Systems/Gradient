use gradient_compiler::lexer::Lexer;

fn main() {
    let source = std::fs::read_to_string("/tmp/smoke_concat.gr").unwrap();
    let mut lexer = Lexer::new(&source, 0);
    let tokens = lexer.tokenize();
    
    // Find tokens around line 3529
    for (i, t) in tokens.iter().enumerate() {
        if t.span.start.line >= 3525 && t.span.start.line <= 3535 {
            if i % 100 == 0 {  // Sample every 100th token to reduce output
                println!("Token {} at line {}: {:?}", i, t.span.start.line, t.kind);
            }
        }
    }
}
