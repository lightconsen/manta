import type { ReactNode } from "react";

interface ModalProps {
  children: ReactNode;
}

/** Centered overlay dialog shell. Headings/body are provided by the caller. */
export function Modal({ children }: ModalProps) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-card rounded-xl p-6 max-w-md w-full mx-4 shadow-xl">{children}</div>
    </div>
  );
}
