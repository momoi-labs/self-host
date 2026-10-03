import { join, split } from "shlex";

export function nativeCommandFields(command: readonly string[] = []) {
  return { command: join(command) };
}

export function nativeCommand(fields: { command: string }): string[] {
  let command: string[];
  try {
    command = split(fields.command);
  } catch {
    throw new Error("Check the quotes and escapes in the start command.");
  }
  if (!command[0]) throw new Error("Enter a start command.");
  return command;
}
