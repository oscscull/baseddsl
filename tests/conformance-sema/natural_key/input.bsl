# `@key(field)` nominates a declared column as the primary key — no surrogate `id` is
# synthesized. The natural key is app-supplied (a `sku`/`iso_code`, not engine-generated),
# so the create reads its row back keyed on that column, and an inbound relation references
# the nominated column with its real type.
@key(iso_code)
Country {
  iso_code: text
  name:     text
}

Customer {
  id:      Id
  country: Country
  email:   text
}

shape CountryRow from Country { iso_code, name }
shape CustomerRow from Customer { id, email, country = country.iso_code }

query country(iso_code) -> CountryRow;
query customer(id) -> CustomerRow;

mutation add_country(iso_code: text, name: text) -> CountryRow {
  create Country { iso_code = $iso_code, name = $name };
}
