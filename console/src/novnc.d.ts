/** The part of noVNC's client the console uses. The package ships no types. */
declare module "@novnc/novnc" {
  export type Credentials = { username?: string; password?: string; target?: string };

  export default class RFB extends EventTarget {
    constructor(target: HTMLElement, urlOrChannel: string | WebSocket, options?: { shared?: boolean; credentials?: Credentials });
    scaleViewport: boolean;
    resizeSession: boolean;
    background: string;
    focus(): void;
    disconnect(): void;
    sendCredentials(credentials: Credentials): void;
  }
}
