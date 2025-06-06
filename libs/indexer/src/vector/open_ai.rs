use app_error::AppError;
use appflowy_ai_client::dto::EmbeddingModel;
use async_openai::config::{AzureConfig, Config, OpenAIConfig};
use async_openai::types::{CreateEmbeddingRequest, CreateEmbeddingResponse};
use async_openai::Client;
use tiktoken_rs::CoreBPE;
use tracing::trace;

pub const OPENAI_EMBEDDINGS_URL: &str = "https://api.openai.com/v1/embeddings";

pub const REQUEST_PARALLELISM: usize = 40;

#[derive(Debug, Clone)]
pub struct OpenAIEmbedder {
  pub(crate) client: Client<OpenAIConfig>,
}

impl OpenAIEmbedder {
  pub fn new(config: OpenAIConfig) -> Self {
    let client = Client::with_config(config);

    Self { client }
  }
}

#[derive(Debug, Clone)]
pub struct AzureOpenAIEmbedder {
  pub(crate) client: Client<AzureConfig>,
}

impl AzureOpenAIEmbedder {
  pub fn new(mut config: AzureConfig) -> Self {
    // Make sure your Azure AI service support the model
    config = config.with_deployment_id(EmbeddingModel::default_model().to_string());
    let client = Client::with_config(config);
    Self { client }
  }
}

pub async fn async_embed<C: Config>(
  client: &Client<C>,
  request: CreateEmbeddingRequest,
) -> Result<CreateEmbeddingResponse, AppError> {
  trace!(
    "async embed with request: model:{:?}, dimension:{:?}, api_base:{}",
    request.model,
    request.dimensions,
    client.config().api_base()
  );
  let response = client
    .embeddings()
    .create(request)
    .await
    .map_err(|err| AppError::Unhandled(err.to_string()))?;
  Ok(response)
}

/// ## Execution Time Comparison Results
///
/// The following results were observed when running `execution_time_comparison_tests`:
///
/// | Content Size (chars) | Direct Time (ms) | spawn_blocking Time (ms) |
/// |-----------------------|------------------|--------------------------|
/// | 500                  | 1                | 1                        |
/// | 1000                 | 2                | 2                        |
/// | 2000                 | 5                | 5                        |
/// | 5000                 | 11               | 11                       |
/// | 20000                | 49               | 48                       |
///
/// ## Guidelines for Using `spawn_blocking`
///
/// - **Short Tasks (< 1 ms)**:
///   Use direct execution on the async runtime. The minimal execution time has negligible impact.
///
/// - **Moderate Tasks (1–10 ms)**:
///   - For infrequent or low-concurrency tasks, direct execution is acceptable.
///   - For frequent or high-concurrency tasks, consider using `spawn_blocking` to avoid delays.
///
/// - **Long Tasks (> 10 ms)**:
///   Always offload to a blocking thread with `spawn_blocking` to maintain runtime efficiency and responsiveness.
///
/// Related blog:
/// https://tokio.rs/blog/2020-04-preemption
/// https://ryhl.io/blog/async-what-is-blocking/
#[inline]
#[allow(dead_code)]
pub fn split_text_by_max_tokens(
  content: String,
  max_tokens: usize,
  tokenizer: &CoreBPE,
) -> Result<Vec<String>, AppError> {
  if content.is_empty() {
    return Ok(vec![]);
  }

  let token_ids = tokenizer.encode_ordinary(&content);
  let total_tokens = token_ids.len();
  if total_tokens <= max_tokens {
    return Ok(vec![content]);
  }

  let mut chunks = Vec::new();
  let mut start_idx = 0;
  while start_idx < total_tokens {
    let mut end_idx = (start_idx + max_tokens).min(total_tokens);
    let mut decoded = false;
    // Try to decode the chunk, adjust end_idx if decoding fails
    while !decoded {
      let token_chunk = &token_ids[start_idx..end_idx];
      // Attempt to decode the current chunk
      match tokenizer.decode(token_chunk.to_vec()) {
        Ok(chunk_text) => {
          chunks.push(chunk_text);
          start_idx = end_idx;
          decoded = true;
        },
        Err(_) => {
          // If we can extend the chunk, do so
          if end_idx < total_tokens {
            end_idx += 1;
          } else if start_idx + 1 < total_tokens {
            // Skip the problematic token at start_idx
            start_idx += 1;
            end_idx = (start_idx + max_tokens).min(total_tokens);
          } else {
            // Cannot decode any further, break to avoid infinite loop
            start_idx = total_tokens;
            break;
          }
        },
      }
    }
  }

  Ok(chunks)
}

pub fn group_paragraphs_by_max_content_len(
  paragraphs: Vec<String>,
  max_content_len: usize,
) -> Vec<String> {
  if paragraphs.is_empty() {
    return vec![];
  }

  let mut result = Vec::new();
  let mut current = String::new();
  for paragraph in paragraphs {
    if paragraph.len() + current.len() > max_content_len {
      // if we add the paragraph to the current content, it will exceed the limit
      // so we push the current content to the result set and start a new chunk
      let accumulated = std::mem::replace(&mut current, paragraph);
      if !accumulated.is_empty() {
        result.push(accumulated);
      }
    } else {
      // add the paragraph to the current chunk
      current.push_str(&paragraph);
    }
  }

  if !current.is_empty() {
    result.push(current);
  }

  result
}

#[cfg(test)]
mod tests {

  use crate::vector::open_ai::{group_paragraphs_by_max_content_len, split_text_by_max_tokens};
  use tiktoken_rs::cl100k_base;

  #[test]
  fn test_split_at_non_utf8() {
    let max_tokens = 10; // Small number for testing

    // Content with multibyte characters (emojis)
    let content = "Hello 😃 World 🌍! This is a test 🚀.".to_string();
    let tokenizer = cl100k_base().unwrap();
    let params = split_text_by_max_tokens(content.clone(), max_tokens, &tokenizer).unwrap();
    for content in params {
      assert!(content.is_char_boundary(0));
      assert!(content.is_char_boundary(content.len()));
    }

    let params = group_paragraphs_by_max_content_len(vec![content], max_tokens);
    for content in params {
      assert!(content.is_char_boundary(0));
      assert!(content.is_char_boundary(content.len()));
    }
  }
  #[test]
  fn test_exact_boundary_split() {
    let max_tokens = 5; // Set to 5 tokens for testing
    let content = "The quick brown fox jumps over the lazy dog".to_string();
    let tokenizer = cl100k_base().unwrap();
    let params = split_text_by_max_tokens(content.clone(), max_tokens, &tokenizer).unwrap();

    let total_tokens = tokenizer.encode_ordinary(&content).len();
    let expected_fragments = (total_tokens + max_tokens - 1) / max_tokens;
    assert_eq!(params.len(), expected_fragments);
  }

  #[test]
  fn test_content_shorter_than_max_len() {
    let max_tokens = 100;
    let content = "Short content".to_string();
    let tokenizer = cl100k_base().unwrap();
    let params = split_text_by_max_tokens(content.clone(), max_tokens, &tokenizer).unwrap();

    assert_eq!(params.len(), 1);
    assert_eq!(params[0], content);
  }

  #[test]
  fn test_empty_content() {
    let max_tokens = 10;
    let content = "".to_string();
    let tokenizer = cl100k_base().unwrap();
    let params = split_text_by_max_tokens(content.clone(), max_tokens, &tokenizer).unwrap();
    assert_eq!(params.len(), 0);

    let params = group_paragraphs_by_max_content_len(params, max_tokens);
    assert_eq!(params.len(), 0);
  }

  #[test]
  fn test_content_with_only_multibyte_characters() {
    let max_tokens = 1; // Set to 1 token for testing
    let content = "😀😃😄😁😆".to_string();
    let tokenizer = cl100k_base().unwrap();
    let params = split_text_by_max_tokens(content.clone(), max_tokens, &tokenizer).unwrap();

    let emojis: Vec<String> = content.chars().map(|c| c.to_string()).collect();
    for (param, emoji) in params.iter().zip(emojis.iter()) {
      assert_eq!(param, emoji);
    }

    let params = group_paragraphs_by_max_content_len(params, max_tokens);
    for (param, emoji) in params.iter().zip(emojis.iter()) {
      assert_eq!(param, emoji);
    }
  }

  #[test]
  fn test_split_with_combining_characters() {
    let max_tokens = 1; // Set to 1 token for testing
    let content = "a\u{0301}e\u{0301}i\u{0301}o\u{0301}u\u{0301}".to_string(); // "áéíóú"

    let tokenizer = cl100k_base().unwrap();
    let params = split_text_by_max_tokens(content.clone(), max_tokens, &tokenizer).unwrap();
    let total_tokens = tokenizer.encode_ordinary(&content).len();
    assert_eq!(params.len(), total_tokens);
    let reconstructed_content = params.join("");
    assert_eq!(reconstructed_content, content);

    let params = group_paragraphs_by_max_content_len(params, max_tokens);
    let reconstructed_content: String = params.concat();
    assert_eq!(reconstructed_content, content);
  }

  #[test]
  fn test_large_content() {
    let max_tokens = 1000;
    let content = "a".repeat(5000); // 5000 characters
    let tokenizer = cl100k_base().unwrap();
    let params = split_text_by_max_tokens(content.clone(), max_tokens, &tokenizer).unwrap();

    let total_tokens = tokenizer.encode_ordinary(&content).len();
    let expected_fragments = (total_tokens + max_tokens - 1) / max_tokens;
    assert_eq!(params.len(), expected_fragments);
  }

  #[test]
  fn test_non_ascii_characters() {
    let max_tokens = 2;
    let content = "áéíóú".to_string();
    let tokenizer = cl100k_base().unwrap();
    let params = split_text_by_max_tokens(content.clone(), max_tokens, &tokenizer).unwrap();

    let total_tokens = tokenizer.encode_ordinary(&content).len();
    let expected_fragments = (total_tokens + max_tokens - 1) / max_tokens;
    assert_eq!(params.len(), expected_fragments);
    let reconstructed_content: String = params.concat();
    assert_eq!(reconstructed_content, content);

    let params = group_paragraphs_by_max_content_len(params, max_tokens);
    let reconstructed_content: String = params.concat();
    assert_eq!(reconstructed_content, content);
  }

  #[test]
  fn test_content_with_leading_and_trailing_whitespace() {
    let max_tokens = 3;
    let content = "  abcde  ".to_string();
    let tokenizer = cl100k_base().unwrap();
    let params = split_text_by_max_tokens(content.clone(), max_tokens, &tokenizer).unwrap();

    let total_tokens = tokenizer.encode_ordinary(&content).len();
    let expected_fragments = (total_tokens + max_tokens - 1) / max_tokens;
    assert_eq!(params.len(), expected_fragments);
    let reconstructed_content: String = params.concat();
    assert_eq!(reconstructed_content, content);

    let params = group_paragraphs_by_max_content_len(params, max_tokens);
    let reconstructed_content: String = params.concat();
    assert_eq!(reconstructed_content, content);
  }

  #[test]
  fn test_content_with_multiple_zero_width_joiners() {
    let max_tokens = 1;
    let content = "👩‍👩‍👧‍👧👨‍👨‍👦‍👦".to_string();
    let tokenizer = cl100k_base().unwrap();
    let params = split_text_by_max_tokens(content.clone(), max_tokens, &tokenizer).unwrap();
    let reconstructed_content: String = params.concat();
    assert_eq!(reconstructed_content, content);

    let params = group_paragraphs_by_max_content_len(params, max_tokens);
    let reconstructed_content: String = params.concat();
    assert_eq!(reconstructed_content, content);
  }

  #[test]
  fn test_content_with_long_combining_sequences() {
    let max_tokens = 1;
    let content = "a\u{0300}\u{0301}\u{0302}\u{0303}\u{0304}".to_string();
    let tokenizer = cl100k_base().unwrap();
    let params = split_text_by_max_tokens(content.clone(), max_tokens, &tokenizer).unwrap();
    let reconstructed_content: String = params.concat();
    assert_eq!(reconstructed_content, content);

    let params = group_paragraphs_by_max_content_len(params, max_tokens);
    let reconstructed_content: String = params.concat();
    assert_eq!(reconstructed_content, content);
  }
}

// #[cfg(test)]
// mod execution_time_comparison_tests {
//   use crate::indexer::document_indexer::split_text_by_max_tokens;
//   use rand::distributions::Alphanumeric;
//   use rand::{thread_rng, Rng};
//   use std::sync::Arc;
//   use std::time::Instant;
//   use tiktoken_rs::{cl100k_base, CoreBPE};
//
//   #[tokio::test]
//   async fn test_execution_time_comparison() {
//     let tokenizer = Arc::new(cl100k_base().unwrap());
//     let max_tokens = 100;
//
//     let sizes = vec![500, 1000, 2000, 5000, 20000]; // Content sizes to test
//     for size in sizes {
//       let content = generate_random_string(size);
//
//       // Measure direct execution time
//       let direct_time = measure_direct_execution(content.clone(), max_tokens, &tokenizer);
//
//       // Measure spawn_blocking execution time
//       let spawn_blocking_time =
//         measure_spawn_blocking_execution(content, max_tokens, Arc::clone(&tokenizer)).await;
//
//       println!(
//         "Content Size: {} | Direct Time: {}ms | spawn_blocking Time: {}ms",
//         size, direct_time, spawn_blocking_time
//       );
//     }
//   }
//
//   // Measure direct execution time
//   fn measure_direct_execution(content: String, max_tokens: usize, tokenizer: &CoreBPE) -> u128 {
//     let start = Instant::now();
//     split_text_by_max_tokens(content, max_tokens, tokenizer).unwrap();
//     start.elapsed().as_millis()
//   }
//
//   // Measure `spawn_blocking` execution time
//   async fn measure_spawn_blocking_execution(
//     content: String,
//     max_tokens: usize,
//     tokenizer: Arc<CoreBPE>,
//   ) -> u128 {
//     let start = Instant::now();
//     tokio::task::spawn_blocking(move || {
//       split_text_by_max_tokens(content, max_tokens, tokenizer.as_ref()).unwrap()
//     })
//     .await
//     .unwrap();
//     start.elapsed().as_millis()
//   }
//
//   pub fn generate_random_string(len: usize) -> String {
//     let rng = thread_rng();
//     rng
//       .sample_iter(&Alphanumeric)
//       .take(len)
//       .map(char::from)
//       .collect()
//   }
// }
