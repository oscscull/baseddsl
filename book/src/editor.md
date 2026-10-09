# VS Code

Install the matching `based-vscode-VERSION.vsix` from the same artifact set as
`based-lsp`. Check its checksum, then use **Extensions: Install from VSIX**.
Keep the LSP on PATH or set `basedls.serverPath` to its absolute path.
The extension checks the LSP version before starting it. Upgrade or roll back both together.

To work on the extension from source, run `make ci-extension`. It requires
Node/npm and builds, tests, and packages the VSIX. Use the extension development
launch configuration to test changes interactively.
