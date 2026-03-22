# Constrained Decoding with the Gradient Grammar

This guide shows how to use the formal EBNF grammar (`gradient.ebnf`) with
popular constrained-decoding engines so that an LLM can only generate
syntactically valid Gradient source code.

---

## Overview

Constrained decoding restricts a language model's token sampling at each step
so that only tokens consistent with a formal grammar are permitted. This
guarantees that every generated program parses successfully -- no syntax
errors, no hallucinated keywords, no mismatched indentation.

The file `gradient.ebnf` contains a complete, standalone EBNF grammar covering
all Gradient language constructs: modules, imports, function definitions, let
bindings, control flow (`if`/`for`/`while`/`match`), enum declarations, type
aliases, annotations, effect sets, and the `@cap` capability system.

---

## vLLM (`--guided-grammar`)

vLLM supports grammar-constrained generation via the `--guided-grammar` flag
or the `guided_grammar` parameter in the API.

### Server-side (OpenAI-compatible API)

Start the vLLM server as usual:

```bash
vllm serve <model> --host 0.0.0.0 --port 8000
```

Then pass the grammar in your API request:

```python
import requests
from pathlib import Path

grammar = Path("resources/gradient.ebnf").read_text()

response = requests.post("http://localhost:8000/v1/completions", json={
    "model": "<model>",
    "prompt": "Write a Gradient function that computes the absolute value of an integer:\n",
    "max_tokens": 256,
    "extra_body": {
        "guided_grammar": grammar,
    },
})

print(response.json()["choices"][0]["text"])
```

### Offline batch generation

```python
from vllm import LLM, SamplingParams
from pathlib import Path

grammar = Path("resources/gradient.ebnf").read_text()

llm = LLM(model="<model>")
params = SamplingParams(
    max_tokens=256,
    guided_decoding={"grammar": grammar},
)

outputs = llm.generate(
    ["Write a Gradient function that computes factorial:\n"],
    sampling_params=params,
)

print(outputs[0].outputs[0].text)
```

---

## llguidance

[llguidance](https://github.com/guidance-ai/llguidance) is a Rust library
(with Python bindings) for grammar-constrained generation. It accepts EBNF
grammars directly.

```python
import llguidance as llg
from pathlib import Path

grammar_text = Path("resources/gradient.ebnf").read_text()

# Create a grammar constraint from the EBNF
grammar = llg.Grammar.from_ebnf(grammar_text, start="program")

# Use with a tokenizer and model of your choice
tokenizer = llg.Tokenizer.from_pretrained("<model>")
constraint = llg.Constraint(grammar, tokenizer)

# During generation, apply the constraint at each step:
#   allowed_tokens = constraint.get_mask()
#   ... sample from allowed_tokens ...
#   constraint.advance(chosen_token)
```

### Integration with guidance library

```python
from guidance import models, gen
from pathlib import Path

grammar_text = Path("resources/gradient.ebnf").read_text()

model = models.Transformers("<model>")

result = model + "Write a Gradient function:\n" + gen(
    grammar=grammar_text,
    max_tokens=256,
)

print(result)
```

---

## Outlines

[Outlines](https://github.com/dottxt-ai/outlines) supports grammar-guided
generation via its `CFG` sampler.

```python
import outlines
from pathlib import Path

grammar_text = Path("resources/gradient.ebnf").read_text()

model = outlines.models.transformers("<model>")
generator = outlines.generate.cfg(model, grammar_text)

result = generator(
    "Write a Gradient function that checks if a number is even:\n",
    max_tokens=256,
)

print(result)
```

### With vLLM backend in Outlines

```python
import outlines
from pathlib import Path

grammar_text = Path("resources/gradient.ebnf").read_text()

model = outlines.models.vllm("<model>")
generator = outlines.generate.cfg(model, grammar_text)

result = generator(
    "Generate a Gradient module with a match expression:\n",
    max_tokens=512,
)

print(result)
```

---

## Example: Generating a Valid Gradient Function

Below is an example prompt and the kind of output you can expect when
constrained decoding is active. The grammar ensures the output is always
syntactically valid Gradient.

**Prompt:**

```
Write a Gradient function that takes a Direction enum and returns the
opposite direction using a match expression.
```

**Expected output (constrained to gradient.ebnf):**

```
mod compass

type Direction = North | South | East | West

fn opposite(d: Direction) -> Direction:
    match d:
        North:
            ret South
        South:
            ret North
        East:
            ret West
        West:
            ret East

fn main() -> !{IO} ():
    let dir: Direction = North
    let opp: Direction = opposite(dir)
    print("done")
```

Every token in the output was chosen from the set allowed by the grammar at
that decoding step. The result is guaranteed to parse without errors.

---

## Notes on Indentation Handling

Gradient uses significant whitespace with synthetic `INDENT`, `DEDENT`, and
`NEWLINE` tokens. Most constrained-decoding engines operate on raw characters,
not pre-tokenized streams, so you may need to map these synthetic tokens to
their character-level equivalents:

| Grammar token | Character-level equivalent |
|---------------|----------------------------|
| `NEWLINE`     | `\n`                       |
| `INDENT`      | Increase leading spaces by 4 |
| `DEDENT`      | Decrease leading spaces by 4 |
| `EOF`         | End of string              |

Some engines (notably XGrammar and llguidance) support indentation-sensitive
grammars natively. For engines that do not, you may need to expand the INDENT
and DEDENT rules into explicit whitespace patterns, or post-process the grammar
to inline the indentation at each nesting level up to a fixed maximum depth.

A practical approach for engines without native indentation support: define
`block_1`, `block_2`, ... `block_N` rules at each nesting depth, where each
level's statements are prefixed by the appropriate number of spaces. Depth 5
is sufficient for most Gradient programs.
