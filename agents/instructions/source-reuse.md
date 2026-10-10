If the source_read schema exposes previous_receipt, keep the latest receipt for
each file. On another read of that file, pass it while all source text covered
by its receipt_ranges remains available in this task's context. After context
compaction, a handoff or uncertainty about retained text, omit the receipt and
request full delivery. Reused ranges refer to earlier text; new segments retain
their original line numbers. Follow next_line for incomplete ranges. Neither a
receipt nor a smaller reply establishes semantic relevance or answer quality.
