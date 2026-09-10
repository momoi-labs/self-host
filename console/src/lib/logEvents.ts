export type LogLine = { text: string; level?: "info" | "error" };

/** Decode complete SSE events without changing the whitespace in their data. */
export function logEventParser() {
  let buffer = "";
  let event = "message";
  let data: string[] = [];
  return (chunk: string): LogLine[] => {
    buffer += chunk;
    const lines = buffer.split("\n");
    buffer = lines.pop() ?? "";
    const result: LogLine[] = [];
    for (const raw of lines) {
      const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;
      if (!line) {
        if (data.length) result.push(...data.map((text): LogLine => ({
          text, level: event === "notice" ? "info" : undefined,
        })));
        data = [];
        event = "message";
      } else if (!line.startsWith(":")) {
        const colon = line.indexOf(":");
        const field = colon < 0 ? line : line.slice(0, colon);
        const value = colon < 0 ? "" : line.slice(colon + 1).replace(/^ /, "");
        if (field === "event") event = value;
        if (field === "data") data.push(value);
      }
    }
    return result;
  };
}
