import { Apple, Monitor, Terminal, Smartphone, Bot } from "lucide-react";
import type { ReactNode } from "react";

const base = import.meta.env.BASE_URL;

const PLATFORMS: { name: string; icon: ReactNode }[] = [
  { name: "macOS", icon: <Apple className="h-5 w-5" /> },
  { name: "Windows", icon: <Monitor className="h-5 w-5" /> },
  { name: "Linux", icon: <Terminal className="h-5 w-5" /> },
  { name: "iOS", icon: <Smartphone className="h-5 w-5" /> },
  { name: "Android", icon: <Bot className="h-5 w-5" /> },
];

function PhoneShot({ light, dark, alt }: { light: string; dark: string; alt: string }) {
  return (
    <figure className="card relative rounded-3xl p-2.5">
      <div className="mx-auto mb-2 h-1 w-16 rounded-full bg-line" aria-hidden="true" />
      <picture>
        <source media="(prefers-color-scheme: dark)" srcSet={dark} />
        <source media="(prefers-color-scheme: light)" srcSet={light} />
        <img
          src={light}
          alt={alt}
          loading="lazy"
          className="h-[420px] w-full rounded-2xl object-cover"
        />
      </picture>
    </figure>
  );
}

export default function Platforms() {
  return (
    <section id="platforms" className="mx-auto max-w-6xl px-6 py-24">
      <div className="mb-12 text-center">
        <h2 className="text-3xl font-bold tracking-tight sm:text-4xl">
          Every device, <span className="text-gradient">one agent</span>
        </h2>
        <p className="mx-auto mt-4 max-w-2xl text-muted">
          The same local runtime and memory on every machine you own. Chat,
          voice, camera, and device tools on your phone — full desktop control
          on your computer.
        </p>
      </div>

      <div className="mb-14 flex flex-wrap items-center justify-center gap-3">
        {PLATFORMS.map((p) => (
          <span
            key={p.name}
            className="inline-flex items-center gap-2 rounded-full border border-line bg-panel px-4 py-2 text-sm font-medium text-ink/90"
          >
            <span className="text-brand-400">{p.icon}</span>
            {p.name}
          </span>
        ))}
      </div>

      <div className="grid gap-6 sm:grid-cols-2">
        <PhoneShot
          light={`${base}assets/mobile-ios-light.png`}
          dark={`${base}assets/mobile-ios-dark.png`}
          alt="Syscity on iOS"
        />
        <PhoneShot
          light={`${base}assets/mobile-android-light.png`}
          dark={`${base}assets/mobile-android-dark.png`}
          alt="Syscity on Android"
        />
      </div>
    </section>
  );
}
