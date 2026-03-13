export function escapeHtml(text: string): string {
  const div = document.createElement('div');
  div.textContent = text;
  return div.innerHTML;
}

export function formatContent(content: string): string {
  let formatted = escapeHtml(content);

  // Code blocks
  formatted = formatted.replace(
    /```(\w+)?\n([\s\S]*?)```/g,
    '<div class="code-block"><pre><code>$2</code></pre></div>'
  );

  // Inline code
  formatted = formatted.replace(/`([^`]+)`/g, '<code>$1</code>');

  // Bold
  formatted = formatted.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');

  // Italic
  formatted = formatted.replace(/\*([^*]+)\*/g, '<em>$1</em>');

  // Newlines
  formatted = formatted.replace(/\n/g, '<br>');

  return formatted;
}
