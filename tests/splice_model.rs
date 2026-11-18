//! The splice against a real model (docs/design.md, "The bank"): the entity's text taken as a bank and loaded gives the same
//! answer as reading the text, another entity's bank does not, and the bank run reads fewer tokens.
//! Needs a Qwen3 chat model file: MEMBRAMA_TEST_MODEL=<path to the gguf>. Without it the test says so and passes, so CI never needs a model.
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use membrama::engine::Runner;

const PRE: &str = "<|im_start|>user\nContext about the entity:";
const ZORVEX: &str = " Zorvex lives in Lima. Zorvex keeps a pet hamster. Zorvex plays the clarinet.";
const KELMAR: &str = " Kelmar lives in Tromso. Kelmar keeps a pet ferret. Kelmar plays the mandolin.";

fn question(q: &str) -> String { format!("\nAnswer in a few words. {q}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n") }

#[test]
fn a_banked_entity_answers_like_its_text_and_reads_less() {
    let Ok(path) = std::env::var("MEMBRAMA_TEST_MODEL") else {
        eprintln!("SKIPPED: MEMBRAMA_TEST_MODEL is not set, so the splice was NOT tested against a model");
        return;
    };
    let backend = LlamaBackend::init().expect("backend");
    let model = LlamaModel::load_from_file(&backend, &path, &LlamaModelParams::default().with_n_gpu_layers(99)).expect("model");
    let mut r = Runner::new(&model, &backend, 2048).expect("runner");

    let zorvex = r.make_bank(ZORVEX).expect("bank");
    let kelmar = r.make_bank(KELMAR).expect("bank");
    assert!(zorvex.tokens > 10 && !zorvex.state.is_empty(), "a bank holds the entity's tokens and its state");

    for (q, want) in [("Where does it live?", "lima"), ("What pet does it keep?", "hamster")] {
        r.reset(); let d0 = r.decoded();
        r.feed_text(PRE).unwrap(); r.feed_text(ZORVEX).unwrap(); r.feed_text(&question(q)).unwrap();
        let text = r.generate(12).unwrap(); let text_read = r.decoded() - d0;

        r.reset(); let d1 = r.decoded();
        r.feed_text(PRE).unwrap(); r.splice(&zorvex).unwrap(); r.feed_text(&question(q)).unwrap();
        let banked = r.generate(12).unwrap(); let bank_read = r.decoded() - d1;

        r.reset();
        r.feed_text(PRE).unwrap(); r.splice(&kelmar).unwrap(); r.feed_text(&question(q)).unwrap();
        let wrong = r.generate(12).unwrap();

        assert!(text.to_lowercase().contains(want), "text run should answer {want}: {text:?}");
        assert!(banked.to_lowercase().contains(want), "bank run should answer {want}: {banked:?}");
        assert!(!wrong.to_lowercase().contains(want), "another entity's bank must not answer {want}: {wrong:?}");
        assert!(bank_read < text_read, "the bank run reads fewer tokens ({bank_read} against {text_read})");
    }
}
