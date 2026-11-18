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
| Three similar banks glued together | 26/60 (first bank 4/20, middle 10/20, last 12/20); recomputing the first 16 of about 24 tokens of each later bank gets 47/60, which gives the saving back |
| Soft tokens, nudged vectors, a residual at the name token, the last token's hidden state | all about 0 to 1 out of 60, like a random vector |

So: **one or two banks work, three or more do not**, and nothing but the model's own computed state is read as content. The design is built on that and on nothing else.

## Two states, two owners

- **The working set** (which entities are hot now: people, places, things, newest first) is **the seat's**. It is graph state, the same whatever model is behind it, filled from recall and from what the turn touched, and overwritable by the model. The pronoun getters on `Turn` (`it`, `he`, `there`, `they`) are views over it. membrama never writes it.
- **The bank store and the loaded map** are **membrama's, per model**: which banks exist for this model, how big they are, when each was last used, and which banks occupy which position range in the live sequence. A change of model means a different store, because a bank is valid for one model only.

membrama reads the seat's working set (through the turn request, below) and keeps its own per-model state derived from it.

## The turn, over GraphLink

A node per direction, as the provider-module plan says: the seat owns the request, membrama owns the reply, so no seat-to-peer write is needed.

- Namespace `membrama::`. `membrama::Server:<name>` is a model server (model file, context size); `membrama::Turn:<id>` is one turn.
- **Request** (seat-owned): the model server, the prompt as an ordered list of **segments**, sampling settings, an abort flag. A segment is either plain text or an **entity segment**: `{eid, version, text}`. `version` is the entity's graph version stamp when the text was rendered; `text` is exactly what the seat would have put in the prompt.
- **Reply** (membrama-owned): chunk nodes (a cell value is at most 4096 bytes, so text is chunked), then `done` with counts (prompt tokens read, tokens loaded from banks, tokens generated) or `error`.
- **The module never changes the prompt.** An entity segment may be served from a bank or read as text; the tokens the model sees are the same. If anything about a bank is doubtful (missing, stale, too many), the text is read.

## The bank

- **Key:** model identity (file hash), entity id, entity version, and a hash of the rendered text. Any change of one of them is a different bank; an old bank is dropped when its entity version moves on.
- **Content:** the sequence state of the text computed alone from position 0, with the label inside (`Context about the entity:` and the facts, as in the 27.9-token row), taken with `llama_state_seq_get_data`.
- **Splice:** `llama_state_seq_set_data` into a spare sequence, `llama_memory_seq_add` to shift it to its position, `llama_memory_seq_cp` into the live sequence, `llama_memory_seq_rm` the spare. Requires `llama_memory_can_shift`; a model that cannot shift gets text only.
- **Limit of two:** at most two entity segments in a request are served from banks. More are read as text. This is the measured limit, not a tuned number; the owner can move it.
- **Placement:** banks must be contiguous and each bank's text must not depend on what precedes it. The seat decides the order; the most relevant entity last (recency was the strongest effect in the three-bank test).
- **Reuse across turns:** if the live sequence already holds the same bank at the same place, it stays; otherwise the sequence is rebuilt from the segments. No attempt to patch a sequence in the middle (llama.cpp refuses decoding at positions inside a sequence).

## Budget

The bank store has a size cap and evicts the least recently used. For the trial the cap is **2 GB** (the owner accepted it). One token of `qwen3:8b` is about 150 KB of K and V, so a 25-token bank is a few MB and the cap holds hundreds of entities. Banks live in memory first; whether they are written to disk is open.

## What this does not do

- No vectors from the seat's embedding field go into the model. The field is the seat's recall index only.
- No soft tokens, residual patching or KV mixing across more than two banks.
- No change to what the model is told. It does not recall, rank or rewrite anything.

## Open points (not decided here)

1. **How llama.cpp is bound: decided.** The `llama-cpp-2` crate, pinned (`=0.1.161`), default features off, `metal` on macOS. It exposes the sequence-state calls as bytes (`state_seq_get_size_ext`, `state_seq_get_data_ext`, `state_seq_set_data_ext`) and `kv_cache_seq_add/cp/rm`; there is no `can_shift`, so a model whose cache cannot shift shows up as an error from `kv_cache_seq_add` and the text is read instead. `tests/splice_model.rs` checks the splice against a real Qwen3 file (`MEMBRAMA_TEST_MODEL`); without it the test says it was skipped.
2. **How the seat starts the module.** As a supervised peer through the `peers` fact, like the bridge. The supervision numbers are the owner's.
3. **Disk persistence of banks and how the trial cap is enforced** (count tokens or bytes).
4. **A model whose cache cannot shift** (recurrent models): text only, no banks.
5. **Measure first on a real banner:** what share of a real turn's prefill the focused entities take. The 50% in the test is of a very small prompt.
6. **What fills the seat's working set** is a separate change in the seat (nothing writes `Turn.touched`, `Ctx.frames` or `Ctx.promoted` today).

## Order of work

1. Bank store (keys, size accounting, eviction, invalidation), tested without a model.
2. The splice against a real model, tested with an environment variable naming a model file (the test skips, loudly, when it is not set), checking that a banked entity gives the same answer as its text.
3. The GraphLink peer: turn request in, reply chunks out, text-only first, then banks.
4. The seat side: the segment list in the request, behind a fact, parity with the existing path before it replaces it.
