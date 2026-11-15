# report.json

Read every `.json` file in `data/`. Each holds one region's sales:
`{"region": ..., "items": [{"sku", "qty", "unit_price"}, ...]}`, with
the price as a decimal string. A region can be spread over several
files. Write `report.json` in the project root:

```json
{
  "regions": [
    {"region": "east", "units": 0, "revenue": "0.00", "sources": ["data/sales-east.json"]}
  ],
  "totals": {"units": 0, "revenue": "0.00"},
  "sources": ["data/sales-east.json"]
}
```

- `regions`: one entry per region, sorted by region name, including
  regions with no sales. `units` is the sum of `qty`; `revenue` is the
  sum of `qty * unit_price` as a string with exactly two decimals;
  `sources` lists the files the region's figures came from.
- `totals`: the same two figures over all regions.
- `sources`: every data file read.
- Paths are relative to the project root with `/` separators; every
  list of paths is sorted.
