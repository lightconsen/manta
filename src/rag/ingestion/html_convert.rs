//! HTML to Markdown conversion utility.
//!
//! Extracted from `src/tools/web.rs` for sharing between the KB URL loader
//! and the web fetch tool. Uses simple regex-like replacements — no external
//! HTML parser.

/// Convert HTML to Markdown using simple regex-like replacements.
pub fn html_to_markdown(html: &str) -> String {
    let mut markdown = html.to_string();

    // Remove script and style tags with content
    markdown = remove_tag(&markdown, "script");
    markdown = remove_tag(&markdown, "style");

    // Convert headers
    markdown = replace_tag(&markdown, "h1", "# ");
    markdown = replace_tag(&markdown, "h2", "## ");
    markdown = replace_tag(&markdown, "h3", "### ");
    markdown = replace_tag(&markdown, "h4", "#### ");
    markdown = replace_tag(&markdown, "h5", "##### ");
    markdown = replace_tag(&markdown, "h6", "###### ");

    // Convert formatting
    markdown = replace_tag(&markdown, "strong", "**");
    markdown = replace_tag(&markdown, "b", "**");
    markdown = replace_tag(&markdown, "em", "_");
    markdown = replace_tag(&markdown, "i", "_");
    markdown = replace_tag(&markdown, "code", "`");

    // Convert paragraphs and breaks
    markdown = markdown.replace("<p>", "\n\n");
    markdown = markdown.replace("</p>", "");
    markdown = markdown.replace("<br>", "\n");
    markdown = markdown.replace("<br/>", "\n");
    markdown = markdown.replace("<br />", "\n");

    // Convert lists
    markdown = replace_list_items(&markdown, "li", "- ");
    markdown = markdown.replace("<ul>", "\n");
    markdown = markdown.replace("</ul>", "");
    markdown = markdown.replace("<ol>", "\n");
    markdown = markdown.replace("</ol>", "");

    // Convert links
    markdown = convert_links(&markdown);

    // Remove remaining HTML tags
    markdown = strip_remaining_tags(&markdown);

    // Clean up whitespace
    markdown = markdown
        .lines()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n");

    // Remove multiple consecutive newlines
    while markdown.contains("\n\n\n") {
        markdown = markdown.replace("\n\n\n", "\n\n");
    }

    markdown.trim().to_string()
}

/// Remove a tag and its content entirely (e.g. `<script>...</script>`).
fn remove_tag(html: &str, tag: &str) -> String {
    let pattern_start = format!("<{}[^>]*>", tag);
    let pattern_end = format!("</{}>", tag);

    let mut result = html.to_string();
    while let Some(start) = find_ignore_ascii_case(&result, &pattern_start) {
        if let Some(end) = find_ignore_ascii_case(&result[start..], &pattern_end) {
            let end_pos = start + end + pattern_end.len();
            result.replace_range(start..end_pos, "");
        } else {
            break;
        }
    }
    result
}

/// Replace HTML tags with markdown equivalents.
fn replace_tag(html: &str, tag: &str, replacement: &str) -> String {
    let start_tag = format!("<{}>", tag);
    let end_tag = format!("</{}>", tag);
    let close_start_tag = format!("<{}/>", tag);

    html.replace(&start_tag, replacement)
        .replace(&end_tag, replacement)
        .replace(&close_start_tag, "")
}

/// Replace list items with markdown list prefixes.
fn replace_list_items(html: &str, tag: &str, prefix: &str) -> String {
    let start_tag = format!("<{}>", tag);
    let end_tag = format!("</{}>", tag);

    let mut result = html.to_string();
    while let Some(start) = find_ignore_ascii_case(&result, &start_tag) {
        if let Some(end) = find_ignore_ascii_case(&result[start..], &end_tag) {
            let content_start = start + start_tag.len();
            let content_end = start + end;
            let content = &result[content_start..content_end];
            let replacement = format!("\n{} {}", prefix, content.trim());
            result.replace_range(start..content_end + end_tag.len(), &replacement);
        } else {
            break;
        }
    }
    result
}

/// Convert `<a href="...">text</a>` to `[text](url)`.
fn convert_links(html: &str) -> String {
    let mut result = html.to_string();
    let mut search_start = 0;

    while let Some(start) = find_ignore_ascii_case(&result[search_start..], "<a ") {
        let actual_start = search_start + start;
        if let Some(href_start) = find_ignore_ascii_case(&result[actual_start..], "href=\"") {
            let href_pos = actual_start + href_start + 6;
            if let Some(href_end) = result[href_pos..].find('"') {
                let url = &result[href_pos..href_pos + href_end];
                if let Some(tag_end) = result[actual_start..].find(">") {
                    let content_start = actual_start + tag_end + 1;
                    if let Some(content_end) =
                        find_ignore_ascii_case(&result[content_start..], "</a>")
                    {
                        let text = &result[content_start..content_start + content_end];
                        let replacement = format!("[{}]({})", text.trim(), url);
                        let full_end = content_start + content_end + 4;
                        result.replace_range(actual_start..full_end, &replacement);
                        search_start = actual_start + replacement.len();
                        continue;
                    }
                }
            }
        }
        search_start = actual_start + 1;
    }

    result
}

/// Strip any remaining HTML tags from the string.
fn strip_remaining_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }

    result
}

/// Case-insensitive ASCII substring search.
fn find_ignore_ascii_case(text: &str, pat: &str) -> Option<usize> {
    text.as_bytes()
        .windows(pat.len())
        .position(|window| window.eq_ignore_ascii_case(pat.as_bytes()))
}
