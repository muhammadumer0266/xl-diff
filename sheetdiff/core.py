from typing import List, Tuple, Optional, Any
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
