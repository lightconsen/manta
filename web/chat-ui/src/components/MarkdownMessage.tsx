import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useState } from "react";

/** Detect bare image/video URLs and convert them to markdown embeds. */
function autoEmbedMedia(text: string): string {
  // Match standalone URLs on their own line (or preceded by whitespace)
  // that end with image or video extensions.
  const imageRe =
    /(^|\s)(https?:\/\/[^\s<>"{}|\\^`[\]]+\.(?:png|jpe?g|gif|webp|svg|bmp|ico))(?=$|\s|[.,;!?])/gi;
  const videoRe =
    /(^|\s)(https?:\/\/[^\s<>"{}|\\^`[\]]+\.(?:mp4|webm|mov|mkv|ogv))(?=$|\s|[.,;!?])/gi;

  return text
    .replace(imageRe, (_m, prefix, url) => `${prefix}![image](${url})`)
    .replace(videoRe, (_m, prefix, url) => `${prefix}<video src="${url}" controls />`);
}

export function MarkdownMessage({ text }: { text: string }) {
  const processed = autoEmbedMedia(text);

  return (
    <div className="prose prose-sm dark:prose-invert max-w-none">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          h1: ({ children }) => (
            <h1 className="text-lg font-bold text-gray-900 dark:text-gray-100 mt-4 mb-2">{children}</h1>
          ),
          h2: ({ children }) => (
            <h2 className="text-base font-bold text-gray-900 dark:text-gray-100 mt-3 mb-2">{children}</h2>
          ),
          h3: ({ children }) => (
            <h3 className="text-sm font-bold text-gray-900 dark:text-gray-100 mt-3 mb-1">{children}</h3>
          ),
          strong: ({ children }) => (
            <strong className="font-semibold text-gray-900 dark:text-gray-100">{children}</strong>
          ),
          code: ({ children, className }) => {
            const isBlock = className?.includes("language-");
            if (isBlock) return <code className={className}>{children}</code>;
            return (
              <code className="px-1.5 py-0.5 rounded-md bg-gray-100 dark:bg-neutral-700 text-gray-800 dark:text-gray-200 text-xs font-mono">
                {children}
              </code>
            );
          },
          pre: ({ children }) => (
            <pre className="rounded-xl border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800/80 p-4 overflow-x-auto my-3 text-xs font-mono leading-relaxed">
              {children}
            </pre>
          ),
          blockquote: ({ children }) => (
            <blockquote className="border-l-3 border-emerald-400 dark:border-emerald-600 pl-4 my-3 text-gray-600 dark:text-gray-400 italic">
              {children}
            </blockquote>
          ),
          ul: ({ children }) => (
            <ul className="list-disc list-inside my-2 space-y-1 text-sm">{children}</ul>
          ),
          ol: ({ children }) => (
            <ol className="list-decimal list-inside my-2 space-y-1 text-sm">{children}</ol>
          ),
          li: ({ children }) => (
            <li className="text-sm text-gray-700 dark:text-gray-300 leading-relaxed">{children}</li>
          ),
          p: ({ children }) => (
            <p className="text-sm text-gray-700 dark:text-gray-300 leading-relaxed mb-2 last:mb-0">{children}</p>
          ),
          a: ({ children, href }) => (
            <a href={href} className="text-emerald-600 dark:text-emerald-400 hover:underline" target="_blank" rel="noopener noreferrer">
              {children}
            </a>
          ),
          hr: () => <hr className="my-4 border-gray-200 dark:border-neutral-700" />,
          table: ({ children }) => (
            <table className="w-full text-sm border-collapse my-3">{children}</table>
          ),
          thead: ({ children }) => (
            <thead className="bg-gray-50 dark:bg-neutral-800">{children}</thead>
          ),
          th: ({ children }) => (
            <th className="border border-gray-200 dark:border-neutral-700 px-3 py-2 text-left text-xs font-semibold text-gray-700 dark:text-gray-300">{children}</th>
          ),
          td: ({ children }) => (
            <td className="border border-gray-200 dark:border-neutral-700 px-3 py-2 text-sm text-gray-600 dark:text-gray-400">{children}</td>
          ),
          img: ImageNode,
          video: VideoNode,
        }}
      >
        {processed}
      </ReactMarkdown>
    </div>
  );
}

function ImageNode({ src, alt }: { src?: string; alt?: string }) {
  const [open, setOpen] = useState(false);

  return (
    <>
      <img
        src={src}
        alt={alt || "image"}
        className="rounded-xl border border-gray-200 dark:border-neutral-700 max-w-full max-h-[400px] object-contain cursor-zoom-in hover:opacity-90 transition"
        onClick={() => setOpen(true)}
        loading="lazy"
      />
      {open && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 cursor-zoom-out"
          onClick={() => setOpen(false)}
        >
          <img src={src} alt={alt || "image"} className="max-w-[90vw] max-h-[90vh] object-contain" />
        </div>
      )}
    </>
  );
}

function VideoNode({ src }: { src?: string }) {
  return (
    <video
      src={src}
      controls
      preload="metadata"
      className="rounded-xl border border-gray-200 dark:border-neutral-700 max-w-full max-h-[400px] my-2"
    />
  );
}
