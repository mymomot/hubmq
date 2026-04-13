/// Module listener — tâches Tokio qui souscrivent à des subjects NATS et traitent
/// les messages entrants (ex : appels LLM, proxys, dispatchers customs).
pub mod llm_openai;
