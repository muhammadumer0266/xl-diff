# xl-diff

[![Build Status](https://img.shields.io/badge/build-passing-brightgreen.svg)]()
[![Language](https://img.shields.io/badge/Language-Rust%20%2F%20Python-orange.svg)]()
[![License](https://img.shields.io/badge/License-MIT-blue.svg)]()

`xl-diff` is an ultra-fast, parallelized Excel sheet reconciliation engine built in Rust using **PyO3** and **Calamine**, natively compiled into a lightweight Python extension module. 

By leveraging Rust's safe concurrency and zero-cost abstractions, `xl-diff` evaluates changes, additions, structural modifications, and deletions across massive workbooks in fractions of a second—bypassing the severe memory overhead and processing latency of standard heavy frameworks.

---

## 🏗️ Why `xl-diff`? Industry Use Cases

In sectors where spreadsheets serve as the primary source of truth, tracking row-level revisions manually introduces risk, human error, and massive operational bottlenecks.

### 📊 Financial & Accounting Systems
* **Bank Reconciliation:** Match transactional records against general ledger entries instantaneously using custom unique reference keys.
* **Audit Logs:** Identify exact modifications to cell data, ledger charts, balancing rows, and multi-currency exchange tables between monthly financial close versions.

### 🚧 Construction & Estimating (QTO & Addendums)
* **Addendum Traversal:** Automatically isolate structural changes, added rows of materials, or modified pricing rows when a new architect addendum or Quantity Take-Off (QTO) worksheet version drops.
* **Scope Tracking:** Pinpoint altered trades, modified cost item quantities, and deleted scope metrics instantly without side-by-side human scanning.

### 🔬 Supply Chain & Enterprise Resource Planning (ERP)
* **Master Data Synchronization:** Compare massive inventory lists, bill of materials (BOM), and supplier pricing structures.

---

## ⚡ Performance Matrix

Unlike standard tools like `pandas` or `openpyxl` that read entire files into heavy Python objects, `xl-diff` reads files into memory at the native system layer and utilizes **Rayon** for multi-threaded parallel computation:

| Metric / Library | `openpyxl` | `pandas` | `xl-diff` (Rust Backend) |
|---|---|---|---|
| **Speed (Large Sheets)** | 🐢 Slow (Single-threaded) | ⏳ Moderate | 🚀 Ultra-Fast (Parallelized) |
| **Memory Footprint** | 🔴 Extremely High | 🟡 High | 🟢 Minimal (Zero-Copy overhead) |
| **Row Alignment** | Manual Indexing Required | Index Merge Overhead | Native Key/Positional Aligners |

---

## 🔧 Installation

### Prerequisites
* Python 3.10 or higher
* [Rust toolchain](https://rustup.rs/) (if building or developing locally)

### Local Development Setup
To build the native extension directly into your local Python virtual environment, clone the repository and run `maturin`:

```bash
# Clone the repository
git clone [https://github.com/your-username/xl-diff.git](https://github.com/your-username/xl-diff.git)
cd xl-diff

# Create and activate your virtual environment
python -m venv .venv
source .venv/bin/activate  # On Windows use: .venv\Scripts\activate

# Install development dependencies
pip install maturin

# Compile and install the Rust extension in editable mode
maturin develop
