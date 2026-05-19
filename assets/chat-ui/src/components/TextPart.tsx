import { MarkdownTextPrimitive } from "@assistant-ui/react-markdown";

export function TextPart() {
  return (
    <MarkdownTextPrimitive
      className="prose prose-sm dark:prose-invert max-w-none"
      containerProps={{
        className: "text-sm text-gray-800 dark:text-gray-200 leading-relaxed",
      }}
    />
  );
}
