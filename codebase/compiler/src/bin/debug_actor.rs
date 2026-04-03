use gradient_compiler::codegen::CraneliftCodegen;
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker;

fn main() {
    let src = r#"
actor Counter:
    state count: Int = 0
    on Init:
        ret ()

fn main() -> !{Actor, IO} ():
    let c = spawn Counter
    print("spawned")
    ret ()
"#;

    // 1. Lex
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.lex();

    // 2. Parse
    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    if !parse_errors.is_empty() {
        println!("parse errors: {:?}", parse_errors);
        return;
    }

    // 3. Type check
    let type_errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
    if !real_errors.is_empty() {
        println!("type errors: {:?}", real_errors);
        return;
    }

    // 4. Build IR
    let (ir_module, ir_errors) = IrBuilder::build_module(&ast_module);
    if !ir_errors.is_empty() {
        println!("IR errors: {:?}", ir_errors);
        return;
    }

    // Print IR
    println!("=== Generated IR ===");
    for func in &ir_module.functions {
        println!("\nFunction: {} (extern: {})", func.name, func.extern_lib.is_some());
        println!("  Params: {:?}", func.params);
        println!("  Return: {:?}", func.return_type);
        for (i, block) in func.blocks.iter().enumerate() {
            println!("  Block {} (label: {:?}):", i, block.label);
            for instr in &block.instructions {
                println!("    {:?}", instr);
            }
        }
        println!("  Value types: {:?}", func.value_types);
    }

    // 5. Codegen
    let mut cg = CraneliftCodegen::new().expect("CraneliftCodegen::new");
    if let Err(e) = cg.compile_module(&ir_module) {
        println!("compile_module error: {:?}", e);
        return;
    }
    
    println!("\n=== Codegen succeeded ===");
}
