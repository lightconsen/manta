/** New Session welcome page content (the message area shown when the sidebar
 *  New Session toggle is armed). Presentational only — the composer below it
 *  is the chat area's own component, provided by ChatContent, so the first
 *  message sent here runs through the normal runtime path (which creates the
 *  real session lazily; see transport.armNewSession / run()). */
export function NewSessionWelcome() {
  return (
    <div className="text-center">
      <img
        src="/syscity.png"
        alt="Syscity"
        className="w-16 h-16 mx-auto mb-4"
        draggable={false}
      />
      <p className="text-primary text-base font-medium">
        How can I help today?
      </p>
      <p className="text-secondary text-sm mt-1.5">
        Type your message or press / for commands
      </p>
    </div>
  );
}
