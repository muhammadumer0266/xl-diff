from typing import List, Tuple, Optional, Any, Dict
import pandas as pd

def _load(path: str, sheet_name: Optional[str] = None) -> pd.DataFrame:
    if str(path).lower().endswith(('.xls', '.xlsx', '.xlsm')):
        return pd.read_excel(path, sheet_name=sheet_name, dtype=object)
    return pd.read_csv(path, dtype=object)

def _norm(df: pd.DataFrame) -> pd.DataFrame:
    return df.fillna("").astype(str)

Change = Tuple[str, Any, Optional[str], Optional[str], Optional[str]]

def diff_sheets(left: str, right: str, key: Optional[str] = None, sheet_name: Optional[str] = None) -> List[Change]:
    """Return list of changes: (type, row_key_or_index, column, old, new).
    Types: 'added_row','removed_row','modified_cell'.
    """
    L = _norm(_load(left, sheet_name))
    R = _norm(_load(right, sheet_name))

    changes: List[Change] = []
    if key:
        L2 = L.set_index(key)
        R2 = R.set_index(key)
        all_keys = list(sorted(set(L2.index).union(R2.index)))
        for k in all_keys:
            inL = k in L2.index
            inR = k in R2.index
            if not inL:
                changes.append(("added_row", k, None, None, None))
                continue
            if not inR:
                changes.append(("removed_row", k, None, None, None))
                continue
            lrow = L2.loc[k]
            rrow = R2.loc[k]
            cols = sorted(set(lrow.index).union(rrow.index))
            for c in cols:
                a = str(lrow.get(c, ""))
                b = str(rrow.get(c, ""))
                if a != b:
                    changes.append(("modified_cell", k, c, a, b))
    else:
        maxr = max(len(L), len(R))
        for i in range(maxr):
            if i >= len(L):
                changes.append(("added_row", i, None, None, None)); continue
            if i >= len(R):
                changes.append(("removed_row", i, None, None, None)); continue
            lrow = L.iloc[i]
            rrow = R.iloc[i]
            cols = sorted(set(lrow.index).union(rrow.index))
            for c in cols:
                a = str(lrow.get(c, ""))
                b = str(rrow.get(c, ""))
                if a != b:
                    changes.append(("modified_cell", i, c, a, b))
    return changes

def _is_excel(path: str) -> bool:
    return str(path).lower().endswith(('.xls', '.xlsx', '.xlsm'))

def diff_workbook(left: str, right: str, key: Optional[str] = None) -> Dict[str, List[Change]]:
    """Compare workbooks across all sheets. Returns dict: sheet_name -> changes."""
    # If neither is excel, fallback to single-sheet comparison named 'sheet'
    if not (_is_excel(left) or _is_excel(right)):
        return {"sheet": diff_sheets(left, right, key=key)}

    left_sheets = []
    right_sheets = []
    if _is_excel(left):
        left_sheets = pd.ExcelFile(left).sheet_names
    else:
        left_sheets = ["sheet"]
    if _is_excel(right):
        right_sheets = pd.ExcelFile(right).sheet_names
    else:
        right_sheets = ["sheet"]

    all_sheets = list(dict.fromkeys(list(left_sheets) + list(right_sheets)))
    result: Dict[str, List[Change]] = {}
    for s in all_sheets:
        # If a sheet is missing in one workbook, represent as added/removed row marker
        Lpath = left
        Rpath = right
        if s not in left_sheets:
            # left missing: represent as empty file vs sheet in right
            # create an empty CSV temp via pandas for comparison fallback
            empty = pd.DataFrame()
            empty.to_csv(f".tmp_empty_{s}.csv", index=False)
            Lpath = f".tmp_empty_{s}.csv"
        if s not in right_sheets:
            empty = pd.DataFrame()
            empty.to_csv(f".tmp_empty_{s}.csv", index=False)
            Rpath = f".tmp_empty_{s}.csv"
        result[s] = diff_sheets(Lpath, Rpath, key=key, sheet_name=s)
    return result

def format_unified(changes: List[Change]) -> str:
    out: List[str] = []
    for t, r, col, a, b in changes:
        if t == "added_row":
            out.append(f"+ ROW {r}")
        elif t == "removed_row":
            out.append(f"- ROW {r}")
        else:
            out.append(f"- {r} | {col} = {a}")
            out.append(f"+ {r} | {col} = {b}")
    return "\n".join(out)

def format_workbook(changes_map: Dict[str, List[Change]]) -> str:
    parts: List[str] = []
    for sheet, changes in changes_map.items():
        parts.append(f"--- Sheet: {sheet} ---")
        parts.append(format_unified(changes) or "(no changes)")
    return "\n\n".join(parts)
