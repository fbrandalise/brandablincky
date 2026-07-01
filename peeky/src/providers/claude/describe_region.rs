//! One-shot region description. Used by the Alt hotkey: crop the screen
//! under the mouse, ask Claude what's there, show the answer in an
//! on-screen bubble. No tools, no TTS — this is written on screen, not
//! spoken, so the response is collected in full rather than streamed
//! sentence-by-sentence.

use super::Claude;
use futures_util::StreamExt;

impl Claude {
    /// Describe what's visible in `image_b64` (a cropped screenshot region)
    /// in a sentence or two. Returns the full text once the stream ends.
    pub async fn describe_region(
        &self,
        image_b64: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 220,
            "stream": true,
            "system": describe_region_system_prompt(),
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/jpeg",
                            "data": image_b64
                        }
                    },
                    {
                        "type": "text",
                        "text": "Describe what's visible in this screenshot region and \
                                 suggest what the user could do with it."
                    }
                ]
            }]
        });

        let t_send = std::time::Instant::now();
        let response = self
            .apply_auth(self.http.post(&self.endpoint))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        eprintln!(
            "[describe_region] upload + response headers → {:?}",
            t_send.elapsed()
        );

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            crate::upgrade::on_proxy_error(status.as_u16(), &body_text);
            return Err(format!("describe_region API error {}: {}", status, body_text).into());
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut text_content = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let s = std::str::from_utf8(&chunk)?;
            buffer.push_str(s);

            while let Some(idx) = buffer.find("\n\n") {
                let frame: String = buffer.drain(..idx + 2).collect();
                for line in frame.lines() {
                    let Some(data) = line.strip_prefix("data: ") else {
                        continue;
                    };
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                        continue;
                    };
                    if event["type"].as_str() == Some("content_block_delta")
                        && event["delta"]["type"].as_str() == Some("text_delta")
                        && let Some(t) = event["delta"]["text"].as_str()
                    {
                        text_content.push_str(t);
                    }
                }
            }
        }

        eprintln!(
            "[describe_region] stream complete → {:?} ({} chars)",
            t_send.elapsed(),
            text_content.len()
        );
        Ok(text_content)
    }
}

fn describe_region_system_prompt() -> &'static str {
    "You are a desktop voice-assistant helper. A cropped screenshot of the \
region under the user's mouse cursor is attached. Respond in this exact \
shape:\n\
1. One short sentence describing what's visible there.\n\
2. On new lines, up to 2 concrete actions the user could take with what's \
under the cursor, each on its own line starting with a dash, e.g. \"- \
Clique no botão X para fazer Y.\" Skip this part if nothing actionable is \
there (e.g. empty desktop, plain text).\n\
\n\
Write in natural Brazilian Portuguese, with normal accentuation (não, \
botão, é, etc — the on-screen renderer supports full text now). Keep the \
whole answer under 40 words total. No markdown, no preamble like \"Vejo\" \
or \"Isso mostra\", no code blocks."
}
