import { useMemo } from "react";
import { MantaWebSocketTransport } from "./MantaWebSocketTransport";
import { ChatUI } from "./ChatUI";

function App() {
  const transport = useMemo(() => new MantaWebSocketTransport(), []);

  return (
    <ChatUI transport={transport} />
  );
}

export default App;
