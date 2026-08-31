import { Apple } from "lucide-react";
import { useLanguage } from "../i18n";

function WindowsMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" className={className} aria-hidden="true">
      <path d="M0 3.449L9.75 2.1v9.451H0m10.949-9.602L24 0v11.4H10.949M0 12.6h9.75v9.451L0 20.699M10.949 12.6H24V24l-12.9-1.801" />
    </svg>
  );
}

function LinuxMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" className={className} aria-hidden="true">
      <path d="M12.504 0C6.609 0 1.875 5.473 1.875 12.474c0 1.855.369 3.617 1.02 5.225.652 1.652 1.562 3.12 2.703 4.384C6.662 23.238 8.244 24 10.125 24c1.175 0 2.283-.379 3.129-1.047.846.668 1.954 1.047 3.129 1.047 1.881 0 3.463-.762 4.527-1.917 1.141-1.264 2.051-2.732 2.703-4.384.652-1.608 1.02-3.37 1.02-5.225C24 5.473 19.266 0 13.371 0zm-3.645 6.789c-.359.809-1.345 1.365-2.355 1.365-1.01 0-1.996-.556-2.355-1.365-.359-.81-.036-1.524.723-1.59.762-.066 1.437-.57 1.909-1.13.472.56 1.147 1.064 1.909 1.13.76.066 1.083.78.723 1.59z" />
    </svg>
  );
}

function AppleMark({ className }: { className?: string }) {
  return <Apple className={className} />;
}

function AndroidMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" className={className} aria-hidden="true">
      <path d="M17.523 15.3414c-.5511 0-.9993-.4486-.9993-.9997s.4482-.9993.9993-.9993c.5511 0 .9993.4482.9993.9993.0001.5511-.4482.9997-.9993.9997m-11.046 0c-.5511 0-.9993-.4486-.9993-.9997s.4482-.9993.9993-.9993c.5511 0 .9993.4482.9993.9993 0 .5511-.4482.9997-.9993.9997m11.4045-6.02l1.9973-3.4592a.416.416 0 0 0-.1521-.5676.416.416 0 0 0-.5676.1521l-2.0223 3.503C15.6102 8.2439 13.8533 7.8508 12 7.8508s-3.6102.3931-5.1367 1.0989L4.841 5.4467a.4161.4161 0 0 0-.5677-.1521.4157.4157 0 0 0-.1521.5676l1.9973 3.4592C2.6889 11.1867.3432 14.6589 0 18.7618h24c-.3432-4.1029-2.6889-7.5751-6.1185-9.4404" />
    </svg>
  );
}

const DESKTOP = [
  { name: "macOS", icon: AppleMark, url: "https://syscity.net/syscity-macos.dmg" },
  { name: "Windows", icon: WindowsMark, url: "https://syscity.net/syscity-windows.exe" },
  { name: "Linux", icon: LinuxMark, url: "https://syscity.net/syscity-linux.AppImage" },
];

const MOBILE = [
  { name: "iOS", icon: AppleMark, url: "https://apps.apple.com/app/syscity" },
  { name: "Android", icon: AndroidMark, url: "https://play.google.com/store/apps/details?id=net.syscity" },
];

/** Download section: the merged Platforms + Download area — per-platform
 * clients, with desktop builds as direct downloads and mobile linking to the
 * app stores. */
export default function Download() {
  const { t } = useLanguage();

  return (
    <section id="download" className="border-y border-line bg-alt">
      <div className="mx-auto max-w-6xl px-6 py-28">
        <div className="mb-14 text-center">
          <h2 className="text-3xl font-black tracking-tight sm:text-4xl">
            {t.download.title}
          </h2>
          <p className="mt-4 text-xl font-bold text-brand-600">
            {t.download.subtitle}
          </p>
          <p className="mx-auto mt-4 max-w-2xl text-muted">{t.download.lead}</p>
        </div>

        {/* Desktop downloads */}
        <div className="mb-8 flex justify-center gap-3">
          {DESKTOP.map((p) => (
            <a
              key={p.name}
              href={p.url}
              target="_blank"
              rel="noreferrer"
              title={p.name}
              className="card inline-flex items-center justify-center rounded-xl p-3 transition hover:border-brand-500/40 hover:shadow-[0_8px_24px_rgba(178,42,194,0.10)]"
            >
              <p.icon className="h-6 w-6 text-brand-500" />
            </a>
          ))}
        </div>

        {/* Mobile app stores */}
        <div className="mb-8 flex justify-center gap-3">
          {MOBILE.map((p) => (
            <a
              key={p.name}
              href={p.url}
              target="_blank"
              rel="noreferrer"
              title={p.name}
              className="card inline-flex items-center justify-center rounded-xl p-3 transition hover:border-brand-500/40 hover:shadow-[0_8px_24px_rgba(178,42,194,0.10)]"
            >
              <p.icon className="h-6 w-6 text-brand-500" />
            </a>
          ))}
        </div>

        <p className="text-center text-sm text-muted">{t.download.cloudNote}</p>
      </div>
    </section>
  );
}
