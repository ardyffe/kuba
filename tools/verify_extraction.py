"""
Misura l'accuratezza dell'estrazione confrontandola con la verita' di
riferimento in tools/expected/.

Il confronto e' con il PDF trascritto a mano, non con l'output di un altro
modello: e' l'unico modo di dire "questo modello sbaglia meno di quell'altro"
senza girare in tondo.

Uso:
    python tools/verify_extraction.py <invoice_id> tools/expected/invoice1.json

L'abbinamento fra riga attesa e riga estratta avviene per EAN quando c'e'
(cosi' un modello che numera le righe in modo diverso non viene penalizzato) e
per posizione quando l'EAN manca.
"""

import json
import sys
import urllib.request

API = "http://127.0.0.1:3000/api/invoices"


def fetch(invoice_id: str) -> dict:
    with urllib.request.urlopen(f"{API}/{invoice_id}") as response:
        return json.load(response)


def norm_money(value) -> str | None:
    """'8.10', '8.1' e 8.1 devono contare come uguali."""
    if value is None:
        return None
    try:
        return f"{float(str(value).replace(',', '.')):.2f}"
    except ValueError:
        return str(value)


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2

    invoice_id, expected_path = sys.argv[1], sys.argv[2]

    expected = json.load(open(expected_path, encoding="utf-8"))
    actual = fetch(invoice_id)
    lines = actual.get("lines", [])

    print(f"\n=== {expected['file']} ===")
    print(f"stato fattura : {actual.get('status')}")
    if actual.get("error_message"):
        print(f"errore        : {actual['error_message']}")

    # --- testata ---
    header_checks = [
        ("fornitore", expected["supplier_contains"].lower()
         in (actual.get("supplier_name") or "").lower()),
        ("numero", actual.get("invoice_number") == expected["invoice_number"]),
        ("data", actual.get("invoice_date") == expected["invoice_date"]),
        ("valuta", actual.get("currency") == expected["currency"]),
    ]
    print("\n-- testata --")
    for name, ok in header_checks:
        print(f"  {'OK  ' if ok else 'ERR '} {name}")

    # --- conteggio righe ---
    exp_lines = expected["lines"]
    print(f"\n-- righe --\n  attese: {len(exp_lines)}   estratte: {len(lines)}")

    by_ean = {l["ean"]: l for l in lines if l.get("ean")}
    by_no = {l["line_no"]: l for l in lines}

    totals = {"ean": 0, "quantity": 0, "unit_price": 0, "amount": 0, "kind": 0}
    problems = []

    for exp in exp_lines:
        got = by_ean.get(exp["ean"]) if exp["ean"] else by_no.get(exp["line_no"])

        if got is None:
            problems.append(f"riga {exp['line_no']}: non trovata "
                            f"(ean atteso {exp['ean']})")
            continue

        checks = {
            "ean": (got.get("ean") or None) == exp["ean"],
            "quantity": got.get("quantity") == exp["quantity"],
            "unit_price": norm_money(got.get("unit_price")) == norm_money(exp["unit_price"]),
            "amount": norm_money(got.get("amount")) == norm_money(exp["amount"]),
            "kind": got.get("kind") == exp["kind"],
        }
        for field, ok in checks.items():
            if ok:
                totals[field] += 1
            else:
                problems.append(
                    f"riga {exp['line_no']} {field}: atteso {exp[field]!r}, "
                    f"ottenuto {got.get(field)!r}")

    n = len(exp_lines)
    print("\n-- accuratezza per campo --")
    for field, hits in totals.items():
        pct = 100 * hits / n if n else 0
        print(f"  {field:12} {hits:3}/{n}  {pct:5.1f}%")

    overall = 100 * sum(totals.values()) / (5 * n) if n else 0
    print(f"\n  COMPLESSIVA  {overall:5.1f}%")

    if problems:
        print(f"\n-- differenze ({len(problems)}) --")
        for p in problems[:30]:
            print(f"  {p}")

    # Sotto il 95% l'uscita e' diversa da zero: utile se un giorno lo mettiamo in CI.
    return 0 if overall >= 95 else 1


if __name__ == "__main__":
    sys.exit(main())
