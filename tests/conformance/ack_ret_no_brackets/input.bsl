# `ok[]` does not parse: `ok` is the whole (bare) return.
Tag { label: text }

mutation drop_tags(label: text) -> ok[] {
  delete Tag where (label = $label);
}
