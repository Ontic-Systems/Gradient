#!/bin/bash
#
# Actual Bootstrap Execution Script
# Compiles the self-hosted compiler modules using the reference compiler
#

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
GRADIENT_DIR="/home/gray/TestingGround/Gradient"
COMPILER_DIR="$GRADIENT_DIR/compiler"
OUTPUT_DIR="$GRADIENT_DIR/bootstrap_output"

# Modules to compile
MODULES=(
    "types:types.gr"
    "checker:checker.gr"
    "ir:ir.gr"
    "ir_builder:ir_builder.gr"
    "compiler:compiler.gr"
    "bootstrap:bootstrap.gr"
)

# Counters
TOTAL_MODULES=0
PARSED_MODULES=0
TYPECHECKED_MODULES=0
IR_GENERATED_MODULES=0
TOTAL_LINES=0

# Timing
START_TIME=$(date +%s.%N)

# Function to print header
print_header() {
    echo ""
    echo "╔════════════════════════════════════════════════════════════════╗"
    echo "║          GRADIENT ACTUAL BOOTSTRAP EXECUTION                   ║"
    echo "║     Compiling Self-Hosted Compiler with Reference Compiler     ║"
    echo "╚════════════════════════════════════════════════════════════════╝"
    echo ""
}

# Function to check if gradient is available
check_gradient() {
    if ! command -v gradient &> /dev/null; then
        echo -e "${RED}❌ Gradient compiler not found in PATH${NC}"
        echo "   Please ensure 'gradient' is installed and available"
        exit 1
    fi
    
    echo -e "${GREEN}✅ Reference compiler (gradient) found${NC}"
    echo ""
}

# Function to count lines in a file
get_lines() {
    wc -l < "$1" | tr -d ' '
}

# Function to compile a module
compile_module() {
    local name=$1
    local file=$2
    local filepath="$COMPILER_DIR/$file"
    
    TOTAL_MODULES=$((TOTAL_MODULES + 1))
    
    echo -n "  🔄 Compiling $name ... "
    
    # Check file exists
    if [ ! -f "$filepath" ]; then
        echo -e "${RED}FILE NOT FOUND${NC}"
        return 1
    fi
    
    # Count lines
    local lines=$(get_lines "$filepath")
    TOTAL_LINES=$((TOTAL_LINES + lines))
    
    # Try to parse
    local parse_start=$(date +%s.%N)
    if gradient "$filepath" --parse-only 2>/dev/null >/dev/null; then
        PARSED_MODULES=$((PARSED_MODULES + 1))
        local parse_end=$(date +%s.%N)
        local parse_time=$(echo "$parse_end - $parse_start" | bc 2>/dev/null || echo "0")
        
        # Try to type check
        local tc_start=$(date +%s.%N)
        if gradient "$filepath" --typecheck-only 2>/dev/null >/dev/null; then
            TYPECHECKED_MODULES=$((TYPECHECKED_MODULES + 1))
            local tc_end=$(date +%s.%N)
            local tc_time=$(echo "$tc_end - $tc_start" | bc 2>/dev/null || echo "0")
            
            # Try to generate IR
            local ir_start=$(date +%s.%N)
            if gradient "$filepath" --emit-ir 2>/dev/null >"$OUTPUT_DIR/$name.ir"; then
                IR_GENERATED_MODULES=$((IR_GENERATED_MODULES + 1))
                local ir_end=$(date +%s.%N)
                local ir_time=$(echo "$ir_end - $ir_start" | bc 2>/dev/null || echo "0")
                
                echo -e "${GREEN}✅ PASS${NC} (${lines} lines)"
                echo "     Parse: ${parse_time}s | Type: ${tc_time}s | IR: ${ir_time}s"
                return 0
            else
                local ir_end=$(date +%s.%N)
                local ir_time=$(echo "$ir_end - $ir_start" | bc 2>/dev/null || echo "0")
                echo -e "${YELLOW}⚠️  IR FAIL${NC} (${lines} lines)"
                echo "     Parse: ${parse_time}s | Type: ${tc_time}s | IR: ${ir_time}s"
                
                # Save error
                gradient "$filepath" --emit-ir 2>&1 >"$OUTPUT_DIR/${name}_error.log" || true
                return 1
            fi
        else
            local tc_end=$(date +%s.%N)
            local tc_time=$(echo "$tc_end - $tc_start" | bc 2>/dev/null || echo "0")
            echo -e "${YELLOW}⚠️  TYPE FAIL${NC} (${lines} lines)"
            echo "     Parse: ${parse_time}s | Type: ${tc_time}s"
            
            # Save error
            gradient "$filepath" --typecheck-only 2>&1 >"$OUTPUT_DIR/${name}_error.log" || true
            return 1
        fi
    else
        local parse_end=$(date +%s.%N)
        local parse_time=$(echo "$parse_end - $parse_start" | bc 2>/dev/null || echo "0")
        echo -e "${RED}❌ PARSE FAIL${NC} (${lines} lines)"
        echo "     Parse: ${parse_time}s"
        
        # Save error
        gradient "$filepath" --parse-only 2>&1 >"$OUTPUT_DIR/${name}_error.log" || true
        return 1
    fi
}

# Function to print results
print_results() {
    local end_time=$(date +%s.%N)
    local total_time=$(echo "$end_time - $START_TIME" | bc 2>/dev/null || echo "0")
    
    echo ""
    echo "┌────────────────────────────────────────────────────────────────┐"
    echo "│                    BOOTSTRAP RESULTS                           │"
    echo "├────────────────────────────────────────────────────────────────┤"
    
    for module_info in "${MODULES[@]}"; do
        IFS=':' read -r name file <<< "$module_info"
        local status="❌"
        
        if [ -f "$OUTPUT_DIR/$name.ir" ]; then
            status="✅"
        elif [ -f "$OUTPUT_DIR/${name}_error.log" ]; then
            if grep -q "parse" "$OUTPUT_DIR/${name}_error.log" 2>/dev/null; then
                status="❌"
            else
                status="⚠️ "
            fi
        fi
        
        local lines=0
        if [ -f "$COMPILER_DIR/$file" ]; then
            lines=$(get_lines "$COMPILER_DIR/$file")
        fi
        
        printf "│ %-12s │ %s │ %4d lines │\n" "$name" "$status" "$lines"
    done
    
    echo "├────────────────────────────────────────────────────────────────┤"
    echo "│ Summary:                                                       │"
    printf "│   Modules:         %2d parsed / %2d typechecked / %2d IR gen   │\n" \
        "$PARSED_MODULES" "$TYPECHECKED_MODULES" "$IR_GENERATED_MODULES"
    printf "│   Total Lines:     %4d                                        │\n" "$TOTAL_LINES"
    printf "│   Total Time:      %.3fs                                       │\n" "$total_time"
    
    if [ "$IR_GENERATED_MODULES" -eq "$TOTAL_MODULES" ]; then
        echo -e "│   Status:          ${GREEN}✅ BOOTSTRAP SUCCESS${NC}                          │"
    else
        echo -e "│   Status:          ${RED}❌ BOOTSTRAP FAILED${NC}                           │"
    fi
    
    echo "└────────────────────────────────────────────────────────────────┘"
    
    if [ "$IR_GENERATED_MODULES" -eq "$TOTAL_MODULES" ]; then
        echo ""
        echo -e "${GREEN}🎉🎉🎉 SELF-HOSTING BOOTSTRAP COMPLETE! 🎉🎉🎉${NC}"
        echo ""
        echo "All self-hosted compiler modules compiled successfully!"
        echo "The Gradient compiler can now compile itself!"
        echo ""
        echo "Output saved to: $OUTPUT_DIR"
        echo ""
        echo "IR files generated:"
        ls -la "$OUTPUT_DIR"/*.ir 2>/dev/null || echo "   (none)"
    else
        echo ""
        echo -e "${YELLOW}⚠️  Some modules failed to compile${NC}"
        echo ""
        echo "Error logs:"
        for f in "$OUTPUT_DIR"/*_error.log; do
            if [ -f "$f" ]; then
                echo "  - $(basename "$f")"
            fi
        done
        echo ""
        echo "Check the error logs for details: $OUTPUT_DIR"
    fi
}

# Main execution
main() {
    print_header
    
    # Create output directory
    mkdir -p "$OUTPUT_DIR"
    
    # Check gradient
    check_gradient
    
    # Compile each module
    echo "Compiling self-hosted compiler modules..."
    echo ""
    
    for module_info in "${MODULES[@]}"; do
        IFS=':' read -r name file <<< "$module_info"
        compile_module "$name" "$file" || true
    done
    
    # Print results
    print_results
}

# Run main
main "$@"
