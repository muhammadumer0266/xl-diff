import argparse
from .core import diff_sheets, format_unified, diff_workbook, format_workbook

def main():
    p = argparse.ArgumentParser(prog="sheetdiff")
    p.add_argument("left")
    p.add_argument("right")
    p.add_argument("--key", help="column name to use as row key")
    p.add_argument("--sheet", help="sheet name for Excel files")
    p.add_argument("--all-sheets", action="store_true", help="compare all sheets in workbooks")
    args = p.parse_args()
    if args.all_sheets:
        wb = diff_workbook(args.left, args.right, key=args.key)
        print(format_workbook(wb))
    else:
        changes = diff_sheets(args.left, args.right, key=args.key, sheet_name=args.sheet)
        print(format_unified(changes))


if __name__ == "__main__":
    main()
