//! Scoring a text in a context (docs/design.md, "What the model's probabilities tell"): the bits the model needs for the answer to a question, with the facts in front of it, with none,
//! and with another entity's facts. A fact the answer depends on must make the answer cheaper; a fact about someone else must not. Needs MEMBRAMA_TEST_MODEL; without it the test says so and passes.
mod common;
use common::model_path;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use membrama::engine::Runner;

const PRE: &str = "<|im_start|>user\nContext about the entity:";
fn post(q: &str) -> String { format!("\nAnswer in a few words. {q}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n") }

/// (name, the facts, [(question, the answer as the model writes it: capitalized, as an answer to a question)])
const ENTITIES: [(&str, &str, [(&str, &str); 2]); 5] = [
    ("Zorvex", " Zorvex lives in Lima. Zorvex keeps a pet hamster. Zorvex plays the clarinet.", [("Where does Zorvex live?", "Lima"), ("What pet does Zorvex keep?", "Hamster")]),
    ("Kelmar", " Kelmar lives in Tromso. Kelmar keeps a pet ferret. Kelmar plays the mandolin.", [("Where does Kelmar live?", "Tromso"), ("What instrument does Kelmar play?", "Mandolin")]),
    ("Tuvash", " Tuvash lives in Kyoto. Tuvash keeps a pet gecko. Tuvash plays the bassoon.", [("Where does Tuvash live?", "Kyoto"), ("What instrument does Tuvash play?", "Bassoon")]),
    ("Brindle", " Brindle lives in Oslo. Brindle keeps a pet parrot. Brindle plays the harp.", [("What pet does Brindle keep?", "Parrot"), ("Where does Brindle live?", "Oslo")]),
    ("Quillon", " Quillon lives in Cusco. Quillon keeps a pet tortoise. Quillon plays the flute.", [("What instrument does Quillon play?", "Flute"), ("What pet does Quillon keep?", "Tortoise")]),
];

fn bits(r: &mut Runner, facts: Option<&str>, q: &str, answer: &str) -> f32 {
    r.reset();
    r.feed_text(PRE).unwrap();
    if let Some(f) = facts { r.feed_text(f).unwrap(); }
    r.feed_text(&post(q)).unwrap();
    let per_token = r.score(answer).unwrap();
    assert!(!per_token.is_empty(), "the answer has tokens");
    assert!(per_token.iter().all(|b| b.is_finite() && *b >= 0.0), "bits are finite and not negative: {per_token:?}");
    per_token.iter().sum()
}

#[test]
fn a_fact_the_answer_depends_on_makes_the_answer_cheaper_and_a_fact_about_someone_else_does_not() {
    let Some(path) = model_path() else { return };
    let backend = LlamaBackend::init().expect("backend");
    let model = LlamaModel::load_from_file(&backend, &path, &LlamaModelParams::default().with_n_gpu_layers(99)).expect("model");
    let mut r = Runner::new(&model, &backend, 2048).expect("runner");
    let (mut none_sum, mut right_sum, mut wrong_sum, mut n) = (0f32, 0f32, 0f32, 0);
    for (i, (name, facts, qa)) in ENTITIES.iter().enumerate() {
        let other = ENTITIES[(i + 1) % ENTITIES.len()].1;
        for (q, answer) in qa {
            let none = bits(&mut r, None, q, answer);
            let right = bits(&mut r, Some(facts), q, answer);
            let wrong = bits(&mut r, Some(other), q, answer);
            eprintln!("SCORE {name:8} {answer:9} none {none:6.2}  right {right:6.2}  other's facts {wrong:6.2}");
            assert!(right < none, "{name}/{answer}: the entity's own facts must make its answer cheaper ({right} against {none})");
            assert!(right < wrong, "{name}/{answer}: another entity's facts must not do as well ({right} against {wrong})");
            none_sum += none; right_sum += right; wrong_sum += wrong; n += 1;
        }
    }
    eprintln!("SCORE mean over {n} questions: none {:.2} bits, right facts {:.2}, another's facts {:.2}", none_sum / n as f32, right_sum / n as f32, wrong_sum / n as f32);
}
