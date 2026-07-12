# `stream X[]` does not parse: `stream` already means many.
Order { status: text }

query export_orders() -> stream Order[];
