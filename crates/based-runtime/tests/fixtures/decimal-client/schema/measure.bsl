Measure {
  id: Id
  wide: decimal(38, 9)
  fraction: decimal(38, 38)
  optional: decimal(38, 9)?
}

shape MeasureRow from Measure { id, wide, fraction, optional }

mutation create_measure(wide: decimal(38, 9), fraction: decimal(38, 38)) -> MeasureRow {
  create Measure { wide = $wide, fraction = $fraction };
}

query measure_by_id(id) -> MeasureRow;
