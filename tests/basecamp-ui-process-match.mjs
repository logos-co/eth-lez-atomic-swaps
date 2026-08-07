function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function moduleCommand(rows, executable, moduleName) {
  const escaped = escapeRegExp(moduleName);
  const escapedExecutable = escapeRegExp(executable);
  // Bundle wrappers use the public executable name, while Linux process
  // command lines expose the wrapped ELF basename (for example,
  // `.logos_host.elf`). Both are the same Basecamp-owned host path.
  const executableBasename = `(?:${escapedExecutable}|\\.${escapedExecutable}\\.elf)`;
  const expression = new RegExp(
    `(?:^|/)${executableBasename}(?:\\s|$).*--name(?:=|\\s+)${escaped}(?:\\s|$)`,
  );
  return rows.find((row) => expression.test(row.command));
}
