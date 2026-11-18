# membrama: design

Status: design only, no code yet. Every number below is marked as measured (with where), the owner's, or open.

## What it is

membrama is a GraphLink peer that runs a local model through llama.cpp and keeps **KV banks**: the model's own attention state for the text of an entity, computed once and loaded into the cache instead of being read again. It is a speed-up only. With the banks switched off the same text goes in and the answer is the same; a bank adds no knowledge the text did not have.

It exists because the seat cannot do this itself: Ollama's server does not expose the cache, and llama.cpp does (`llama_state_seq_get_data`, `llama_state_seq_set_data`, `llama_memory_seq_add`, `llama_memory_seq_cp`, `llama_memory_seq_rm`, `llama_memory_can_shift`).

## What was measured (so the design only claims this much)

All on `qwen3:8b` through llama.cpp, invented entities with three short facts each (about 25 tokens), greedy decoding. The programs are scratch files, not in this repository.

| Test | Result |
|---|---|
| One entity's bank spliced in after a fixed prefix, then the question | matches the text version (60/60 with the label inside the bank, 34 tokens decoded per question) |
| The same, asked in pronoun form ("Where does it live?", the entity never named), the bank chosen from outside | 60/60; text 60/60; no bank 0/60; another entity's bank 2/58 |
| Tokens decoded per question for those: text / plain bank / bank with its label | 55.8 / 32.6 / 27.9 (compute only; the bank still occupies context) |
| Two or three similar banks in one request (this module, short and longer facts) | the question about one entity is answered with another's, from the last bank; three glued banks scored 26/60 (first 4/20, middle 10/20, last 12/20) earlier; recomputing the first 16 of about 24 tokens of each later bank gets 47/60, which gives the saving back |
| (kept for reference) three similar banks glued together | 26/60 (first bank 4/20, middle 10/20, last 12/20); recomputing the first 16 of about 24 tokens of each later bank gets 47/60, which gives the saving back |
| Soft tokens, nudged vectors, a residual at the name token, the last token's hidden state | all about 0 to 1 out of 60, like a random vector |

So: **one bank in a prompt works; more than one, or one beside other entities' text, does not**, and nothing but the model's own computed state is read as content. The design is built on that and on nothing else.

## Two states, two owners

- **The working set** (which entities are hot now: people, places, things, newest first) is **the seat's**. It is graph state, the same whatever model is behind it, filled from recall and from what the turn touched, and overwritable by the model. The pronoun getters on `Turn` (`it`, `he`, `there`, `they`) are views over it. membrama never writes it.
- **The bank store and the loaded map** are **membrama's, per model**: which banks exist for this model, how big they are, when each was last used, and which banks occupy which position range in the live sequence. A change of model means a different store, because a bank is valid for one model only.

membrama reads the seat's working set (through the turn request, below) and keeps its own per-model state derived from it.

## The turn, over GraphLink

membrama is a peer the seat starts from the `peers` fact (a supervised process). On the same machine the transport is shared memory, three regions per peer (`docs/graph-link.md`, "The local transport"): the control rings `/gl-<name>-ctl`, **the seat's cell table** `/gl-<name>-sc` (the seat is its only writer; membrama maps it read-only) and **membrama's cell table** `/gl-<name>-pc` (membrama is its only writer; the seat maps it read-only and reads a node membrama owns as a replica). membrama is written against the `Transport` trait of `graphlink-core`, so a remote machine (TCP through `graphlink-net`, the way `graphlink-bridge` joins two seats) needs no change to the protocol. The seat never writes into membrama: the request is a seat-owned node and the reply is membrama-owned nodes.

The names below are a proposal for the owner to confirm. A cell value is at most 4096 bytes, so text is cut into parts.

- **Request** (seat-owned, written by the seat, read by membrama from `/gl-<name>-sc`): `membrane::LlmRequest:<id>` with `server` (which `membrama::Server:<name>`), `parts` (the ids of the part nodes in order), `settings` (sampling), `abort` (the seat sets it to stop; membrama reads it, no write toward membrama).
- **Parts** (seat-owned): `membrane::LlmPart:<id>:<n>` with `kind` (`text` or `entity`) and `text`; an entity part also has `entity` (the entity's id; not `eid`, which the VM provides as the node's own id and refuses to write) and `version` (the entity's graph version stamp when the text was rendered). `text` is exactly what the seat would put in the prompt. A text longer than 4096 bytes is several consecutive parts; **an entity whose text does not fit one part is not bankable** and is read as text.
- **Reply** (membrama-owned, in `/gl-<name>-pc`): `membrama::Reply:<id>` with `state` (`running`, `done`, `aborted` or `error`), `chunks` (how many chunk nodes exist), `usage` (prompt tokens read, tokens loaded from banks, tokens generated) and `error`. `membrama::Chunk:<id>:<n>` holds `text`: the reply's text in order, each chunk at most a cell's value cap minus the 3 bytes of the CBOR header (4093 bytes for a 4096 cap), cut on a character. The chunk being filled is rewritten as the text grows (a new revision each time) and a new chunk starts when it is full, so a reply uses one cell per 4 KB and not one per token; `chunks` says how many exist and is raised only after the chunk is written. The seat subscribes to the reply by its id and reads the chunks it has not seen.
- **A chat request** (seat-owned) is the other form of request: `messages` instead of `parts`. `membrane::LlmRequest:<id>` has `server`, `messages` (the ids of message nodes in order), optionally `tools` (the ids of tool nodes) and `settings` (`max_new` required, `enable_thinking` optional). `membrane::LlmMsg:<id>:<n>` has `role`, `content` (a string, or an array of strings when the text is longer than one cell; they are joined) and, on an assistant message that made tool calls, `tool_calls` (OpenAI shape). `membrane::LlmTool:<id>:<n>` has `spec` (the OpenAI function object). membrama renders them with the model's own template and runs the text; the reply is the same as for parts plus `finish` (`stop` or `tool_calls`) and, when the model asked for tools, `tool_calls` (`[{name, arguments}]`, read out of the text; the chunks keep the model's text as it wrote it). A model with no template, or tools for a model whose reply format is not known, is an error reply. The seat writes the request node last: the handler runs as soon as every listed node has its required cells, and the optional `tool_calls` of a message is not waited for. Banks are used only in the `parts` form; a chat request is one text and gets the prefix reuse.
- **`membrama::Server:<name>`** (membrama-owned): the model file, its identity hash, the context size, and the state of the bank store (banks, bytes of the cap). This is also where the seat reads which models exist.
- **The module never changes the prompt.** An entity part may be served from a bank or read as text; the tokens the model sees are the same. If anything about a bank is doubtful (missing, stale, too many, a part that did not fit), the text is read.
- **Speed.** Shared memory has no wake-up signal: the seat notices a new chunk on its next loop turn (up to 50 ms today), so a reply arrives in steps of that size. Faster wake-up is a separate number for the owner.
- **Cleaning up** the reply and chunk nodes of finished requests (who deletes them, and when) is open.

## The bank

- **Key:** model identity (file hash), entity id, entity version, and a hash of the rendered text. Any change of one of them is a different bank; an old bank is dropped when its entity version moves on.
- **Content:** the sequence state of the text computed alone from position 0, with the label inside (`Context about the entity:` and the facts, as in the 27.9-token row), taken with `llama_state_seq_get_data`.
- **Splice:** `llama_state_seq_set_data` into a spare sequence, `llama_memory_seq_add` to shift it to its position, `llama_memory_seq_cp` into the live sequence, `llama_memory_seq_rm` the spare. Requires `llama_memory_can_shift`; a model that cannot shift gets text only.
- **All or nothing, a limit of one:** a request is served from banks only when it has at most one entity part (`bank_plan`; `--max-banks`, default 1 as the owner chose, 0 turns banks off); with more, every part is read as text. Measured in this module on `qwen3:8b` with similar entities and a question that names the entity: all text answers every question; one bank beside other entities' text answers the banked entity's question with another entity's city; two banks answer the first entity's question with the second's. A single bank in a prompt of its own text (the register case) works. The earlier statement that two banks work did not hold here, and the page is corrected.
- **Placement:** the bank sits where the entity's text would, after what precedes it; each bank's text must not depend on what precedes it.
- **Reuse across requests (prefix):** the generator remembers the tokens its live sequence holds. A request with no bank is tokenized, compared with them, and only the tokens after the shared start are read; the rest of the cache is dropped (`kv_cache_seq_rm`) and at least the last token is always read for its logits. The cache after identical tokens is identical, so the answer does not change (tested against a fresh generator). A sequence that has a bank spliced in is not reused (a bank's tokens are not known to the runner): that request resets. Combining a shared prefix with a bank after it is not built.

## One renderer for any model

The seat sends OpenAI-shaped messages and tools; the model needs its own prompt text. Every model file carries its own chat template (Jinja), so membrama renders with that template (`src/template.rs`, minijinja with its Python-compatibility methods) and has no per-model renderer. Checked: the Qwen3 template from the model file renders chats with tools, tool calls and tool results to the format the template defines, and a plain chat renders byte for byte like llama.cpp's own template function. A template minijinja cannot run is an error that names it; there is no fallback guess.

Reading the model's tool calls back out of its reply (`src/toolcall.rs`): the formats differ by model family, and the one known is the Hermes style (`<tool_call>{json}</tool_call>`, which Qwen3 and others use). `detect_format` recognizes it from the model's own template; a model whose format is not recognized gets its tools refused with an error, never a guess. A call that is not JSON, has no name, or is cut off inside the call is an error that says so, not half a call. Arguments left out are `{}`, and arguments written as a JSON text are read. llama.cpp has a universal parser in C++ that the Rust crate does not expose; adding more formats means adding entries here.

## Budget

The bank store has a size cap and evicts the least recently used. For the trial the cap is **2 GB** (the owner accepted it). One token of `qwen3:8b` is about 150 KB of K and V, so a 25-token bank is a few MB and the cap holds hundreds of entities. Banks live in memory first; whether they are written to disk is open.

## Later, not built: models loaded on demand

membrama could keep several models and load one when a request needs it, evicting the least recently used under a memory cap (as the bank store does for banks), each model a `membrama::Server:<name>` node. The graph could drive it: a small model for simple or background turns and a large one for hard turns, and the model the seat's working set suggests loaded ahead of the request. The routing rule would be a number of the owner's, chosen after a measurement.

Considered and not planned: choosing layers for the GPU by graph knowledge (a dense model uses every layer for every token, so there is nothing to choose by), skipping layers by topic (accuracy cost unknown), and loading mixture-of-experts experts by predicted topic (the model's own router decides from its hidden state; MoE models only; research). Per-topic LoRA adapters that are hot-swapped need training first.

## What this does not do

- No vectors from the seat's embedding field go into the model. The field is the seat's recall index only.
- No soft tokens, residual patching or KV mixing across more than two banks.
- No change to what the model is told. It does not recall, rank or rewrite anything.

## Open points (not decided here)

1. **How llama.cpp is bound: decided.** The `llama-cpp-2` crate, pinned (`=0.1.161`), default features off, `metal` on macOS. It exposes the sequence-state calls as bytes (`state_seq_get_size_ext`, `state_seq_get_data_ext`, `state_seq_set_data_ext`) and `kv_cache_seq_add/cp/rm`; there is no `can_shift`, so a model whose cache cannot shift shows up as an error from `kv_cache_seq_add` and the text is read instead. `tests/splice_model.rs` checks the splice against a real Qwen3 file (`MEMBRAMA_TEST_MODEL`); without it the test says it was skipped.
2. **How the seat starts the module: decided.** A supervised peer through the `peers` fact, like the bridge; the supervision numbers are the owner's (5 s wait, 5 restarts, 60 s reset, as for the other peers).
3. **Disk persistence of banks and how the trial cap is enforced** (count tokens or bytes).
4. **A model whose cache cannot shift** (recurrent models): text only, no banks.
5. **Measure first on a real banner:** what share of a real turn's prefill the focused entities take. The 50% in the test is of a very small prompt.
6. **What fills the seat's working set** is a separate change in the seat (nothing writes `Turn.touched`, `Ctx.frames` or `Ctx.promoted` today).

## Order of work

1. Bank store (keys, size accounting, eviction, invalidation), tested without a model.
2. The splice against a real model, tested with an environment variable naming a model file (the test skips, loudly, when it is not set), checking that a banked entity gives the same answer as its text.
3. The GraphLink peer: turn request in, reply chunks out, text-only first, then banks.
4. The seat side: the segment list in the request, behind a fact, parity with the existing path before it replaces it.
