// Validate process identity before starting an LSP session.
export function requireServerVersion(identity: string, expected: string): void {
  const version = /^based-lsp (\S+) \(/.exec(identity.trim())?.[1];
  if (version !== expected) {
    throw new Error(
      `Expected based-lsp ${expected}, received ${version ?? "an unrecognized version"}. ` +
        `Install based-lsp from the same release as this extension.`,
    );
  }
}
