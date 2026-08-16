# xl-diff Edge & Chaos Test Cases (110+)

This list enumerates edge, failure, and worst-case scenarios to exercise the spreadsheet diff engine.

1. Left workbook has sheet `A`, right workbook missing sheet `A`.
2. Right workbook has new sheet `B` not in left.
3. Sheet present but renamed between files.
4. One workbook is CSV, other is XLSX.
5. Different sheet ordering but same names.
6. Very large sheet (500k rows) vs small sheet.
7. Very wide sheet (50k columns) vs narrow.
8. Mixed types in a column (int, float, str, None).
9. Numeric value change 100 -> 100.0 (type mismatch).
10. Floating point near-equality differences (1.0000001 vs 1.0).
11. Date formatted as string vs Excel date serial.
12. Cell contains formula in one file and value in another.
13. Same formula but different calculated values (volatile functions like TODAY()).
14. Shared string table differences causing different internal indices.
15. Blank trailing rows in one file.
16. Leading blank rows inserted.
17. Entirely empty sheet in one file.
18. Header row present in one file, absent in another.
19. Duplicate row keys in keyed comparison.
20. Duplicate column names.
21. Column order changed but same names.
22. Row order shuffled but same keys (sorting differences).
23. Key column contains nulls.
24. Key column contains mixed types.
25. Key column trimmed vs padded whitespace.
26. Cells with leading/trailing whitespace differences.
27. Cells with non-printable control characters.
28. Different text encodings (UTF-8 vs latin1) in CSVs.
29. CSV with inconsistent quoting rules.
30. Cells containing newlines inside values.
31. Very long cell text (>1MB).
32. Formula references to other sheets that are absent.
33. External links in formulas (broken links).
34. Circular references in formulas causing evaluation failures.
35. Cells with error values (#DIV/0!, #N/A).
36. Password-protected workbook.
37. Workbook with VBA/macros altering content on open.
38. Protected sheets with hidden rows/columns.
39. Hidden sheets present in one workbook only.
40. Filtered/sliced views — visible vs hidden rows.
41. Merged cells affecting column alignment.
42. Differing column data types inferred by pandas.
43. Thousands separator or region-specific number formatting.
44. Different locale date formats (MM/DD vs DD/MM).
45. Cells containing JSON or embedded XML strings.
46. Zip bomb style compressed XLSX causing decompression explosion.
47. Malformed XLSX with missing relations.
48. Corrupted sharedStrings.xml causing parse failure.
49. Very large comment blocks in cells.
50. Cell comments/notes changed but values unchanged.
51. Column inserted at index 0 shifting all columns.
52. Row inserted at index 0 shifting all rows.
53. Many single-row additions across sheet (sparse adds).
54. Many single-cell modifications across sheet (hot path).
55. Thousands of tiny diffs causing huge JSON payload.
56. Binary objects embedded (images) changing storage but not cell values.
57. Differing cell formats (currency vs plain) but same numeric value.
58. Cells with localized minus signs or special unicode digits.
59. Mixed right-to-left text vs left-to-right.
60. Formula parse tokenization differences across Excel versions.
61. Date vs datetime with time-of-day differences.
62. Floating-precision differences because of CSV export rounding.
63. Sheet name contains unusual characters (emoji, slashes).
64. Very long sheet names (>31 characters) truncated in Excel.
65. Workbook-level properties changed (author, modified time) only.
66. File renamed between comparisons (shouldn't affect content).
67. Comparing previous vs current zip recompression metadata difference.
68. Files with different newline conventions (CRLF vs LF) in CSVs.
69. CSV files with BOM present vs absent.
70. Missing columns in one file compared to another.
71. Additional columns with entirely null values.
72. Header names with trailing/leading spaces.
73. Mixed-case header names (Id vs id).
74. Numeric keys stored as floats due to read semantics.
75. Scientific notation representation differences (1e6 vs 1000000).
76. Very small numbers near zero (-0.0 vs 0.0).
77. Boolean vs 'TRUE'/'FALSE' string differences.
78. Multiline CSV rows where newline is part of a value.
79. Sheet copied from template causing hidden metadata diffs.
80. Rows with identical fingerprints but different cell-level ordering.
81. Missing merged-cell expansion behavior when reading.
82. Cells with formulas referencing named ranges that moved.
83. Race condition where file is modified while being read.
84. Incomplete upload (truncated file) vs full file.
85. Files with very large number of distinct columns with many NaNs.
86. Comparing compressed archives (zip of CSVs) instead of XLSX.
87. Files encoded with surrogate pairs and invalid UTF sequences.
88. Very large workbook metadata causing memory spikes.
89. Hidden workbook windows/views causing difference only in UI state.
90. Workbook with custom XML parts altering internal structure.
91. Excel binary (.xlsb) vs XLSX differences (unsupported format).
92. CSV files with inconsistent column counts per row.
93. Trailing delimiter adding an extra empty column.
94. Columns with formula arrays (CSE formulas) vs their expanded results.
95. Changing cell precision via formatting tools (rounding vs truncation).
96. Cells containing long lists separated by commas (ambiguous CSV parsing).
97. Files with thousands of small edits intentionally to performance test.
98. Comparing files across different Excel engine versions (Calc differences).
99. Timezone-aware datetime differences when serialized to strings.
100. Null bytes inside cell strings causing parser issues.
101. Multiple sheets where one sheet exists only in left and another only in right.
102. Workbook saved with different compression level (affects binary size).
103. Cells containing base64 blobs that change due to reserialization.
104. Unicode grapheme cluster differences (visually same, different codepoints).
105. Files where the header row changes type (was data, then promoted to header).
106. Large numbers of tiny files compared sequentially (stress on I/O).
107. Very deep nesting of formula dependencies across sheets.
108. File with extremely high column index (Excel limit exceeded on one side).
109. Cells containing SQL queries or injection-like strings that must not be executed.
110. Files with invalid XML characters in shared strings.

Use these scenarios to design unit, integration and fuzz-style monkey tests. Prioritize safety tests (zip bomb, corrupted XML), correctness tests (keyed/positional alignment, merged cells), and performance tests (large sheets, many edits). Automate a subset as PyTest parametrized cases and record failure modes.
