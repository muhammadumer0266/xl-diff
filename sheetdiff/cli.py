import argparse
from .core import diff_sheets, format_unified

def main():
    p = argparse.ArgumentParser(prog="sheetdiff")
    p.add_argument("left")
    p.add_argument("right")
    p.add_argument("--key", help="column name to use as row key")
    p.add_argument("--sheet", help="sheet name for Excel files")
    args = p.parse_args()
    changes = diff_sheets(args.left, args.right, key=args.key, sheet_name=args.sheet)
    print(format_unified(changes))

if __name__ == "__main__":
    main()
