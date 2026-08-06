const base = import.meta.env.BASE_URL;

export default function Demo() {
  return (
    <section className="mx-auto max-w-5xl px-6 pb-24">
      <div className="card overflow-hidden rounded-3xl">
        <div className="flex items-center gap-2 border-b border-line bg-panel-2 px-4 py-3">
          <span className="h-3 w-3 rounded-full bg-[#ff5f57]" aria-hidden="true" />
          <span className="h-3 w-3 rounded-full bg-[#febc2e]" aria-hidden="true" />
          <span className="h-3 w-3 rounded-full bg-[#28c840]" aria-hidden="true" />
          <span className="ml-3 text-xs text-faint">syscity — agent preview</span>
        </div>
        <img
          src={`${base}assets/demo.gif`}
          alt="Syscity demo — an agent generates a markdown report and previews it in a split panel"
          className="w-full"
          loading="lazy"
        />
      </div>
      <p className="mt-4 text-center text-sm text-muted">
        An agent generates a markdown report via <code className="font-mono">write_report</code>,
        then previews it in a split-panel view.
      </p>
    </section>
  );
}
