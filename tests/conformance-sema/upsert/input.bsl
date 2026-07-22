# Upsert: `create … on conflict (target) update { … }`. Clean on a unique target
# whose columns the create sets; the update branch runs the same self-referential
# arithmetic an ordinary update allows. E0250 non-unique target, E0253 soft-delete.
Page {
  id: Id
  path: text (unique)
  hits: int
}

shape PageRow from Page { path, hits }

mutation record_hit(path: text) -> PageRow {
  create Page { path = $path, hits = 1 } on conflict (path) update { hits = hits + 1 };
}

Note {
  id: Id
  slug: text
  body: text
}

shape NoteRow from Note { slug }

mutation upsert_note(slug: text, body: text) -> NoteRow {
  create Note { slug = $slug, body = $body } on conflict (slug) update { body = $body };
}

@soft_delete(deleted_at)
Tag {
  id: Id
  deleted_at: timestamp?
  label: text (unique)
}

shape TagRow from Tag { label }

mutation touch_tag(label: text) -> TagRow {
  create Tag { label = $label } on conflict (label) update { label = $label };
}
