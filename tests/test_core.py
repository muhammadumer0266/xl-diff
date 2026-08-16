import json
from sheetdiff.core import diff_sheets, format_unified
import pandas as pd

def test_positional_diff(tmp_path):
    a = tmp_path / "a.csv"
    b = tmp_path / "b.csv"
    a.write_text("col1,col2\n1,foo\n2,bar\n")
    b.write_text("col1,col2\n1,foo\n2,baz\n3,qux\n")
    changes = diff_sheets(str(a), str(b))
    out = format_unified(changes)
    assert "+ ROW 2" in out or "+ ROW 3" in out
    assert "col2 = bar" in out or "col2 = baz" in out

def test_keyed_diff(tmp_path):
    a = tmp_path / "a.csv"
    b = tmp_path / "b.csv"
    a.write_text("id,name\nA,foo\nB,bar\n")
    b.write_text("id,name\nA,foo\nB,baz\nC,qux\n")
    changes = diff_sheets(str(a), str(b), key="id")
    s = format_unified(changes)
    assert "+ ROW C" in s
    assert "- B | name = bar" in s
