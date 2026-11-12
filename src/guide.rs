/// The server's instructions, which clients show to the model when they connect.
pub fn instructions(
    enable_writes: bool,
    local_embeddings: bool,
    provider_prefixes: &[&str],
) -> String {
    let writes = if enable_writes {
        "Write tools are enabled. Tools such as `delete_collection` and `delete_points` \
         can't be undone, so confirm with the user before you use them."
    } else {
        "This server is read-only. No tool changes data, so don't look for one."
    };
    let local = if local_embeddings {
        "Texts with a `local/<model>` model, such as `local/nomic-embed-text`, are embedded by \
         this server before they reach Qdrant."
    } else {
        "This server has no local embeddings endpoint, so `local/<model>` models won't work."
    };
    let providers = if provider_prefixes.is_empty() {
        "No external provider keys are configured, so models from OpenAI, Cohere, Jina AI, and \
         OpenRouter (`openai/`, `cohere/`, `jinaai/`, `openrouter/`) won't work."
            .to_string()
    } else {
        let models: Vec<String> = provider_prefixes
            .iter()
            .map(|prefix| format!("`{prefix}<model>`"))
            .collect();
        format!(
            "Qdrant Cloud Inference can also use models from external providers, because this \
             server sends your API keys for them: {}, where `<model>` is the provider's own \
             model name. Never ask the user for a key or put one in a tool argument.",
            models.join(", ")
        )
    };
    format!(
        "{}\n## This server's setup\n\n- {writes}\n- {local}\n- {providers}\n",
        include_str!("guide.md")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setup_section_matches_the_configuration() {
        let bare = instructions(false, false, &[]);
        assert!(bare.contains("This server is read-only") && bare.contains("won't work"));
        assert!(bare.contains("No external provider keys are configured"));
        assert!(!bare.contains("can't be undone") && !bare.contains("embedded by this server"));
        assert!(!bare.contains("sends your API keys"));

        let full = instructions(true, true, &["openrouter/", "jinaai/"]);
        assert!(full.contains("can't be undone") && full.contains("embedded by this server"));
        assert!(full.contains("`openrouter/<model>`, `jinaai/<model>`"));
        assert!(!full.contains("This server is read-only"));
        assert!(!full.contains("No external provider keys"));
    }

    #[test]
    fn the_static_guidance_covers_the_tools_and_cloud_inference() {
        let text = instructions(false, false, &[]);
        assert!(text.contains("request_body") && text.contains("get_collections"));
        assert!(text.contains("Every _managed_ Qdrant Cloud cluster has Cloud Inference"));
        assert!(text.contains("free cluster can use the free models"));
        assert!(text.contains("self-hosted Qdrant has no Cloud Inference"));
    }
}
