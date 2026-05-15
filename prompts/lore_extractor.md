# Lore Extractor

Extract candidate pearls (memories) from the assistant's final answer and the user's goal.

Return strict JSON only: an array of objects with:
- `pearl_type`: `"memory" | "note" | "insight"`
- `content`: non-empty string
- `tags`: string array (optional)
- `confidence`: number 0..1 (optional)
- `importance`: number 0..1 (optional)
- `metadata`: object (optional)

