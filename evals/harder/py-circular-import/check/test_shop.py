import ast, pathlib, subprocess, sys

out = subprocess.run([sys.executable, "-m", "shop"], capture_output=True, text=True)
assert out.returncode == 0, out.stderr
assert out.stdout == open("expected.txt").read(), out.stdout
modules = [p for p in pathlib.Path("shop").glob("*.py") if p.name not in ("__init__.py", "__main__.py")]
assert len(modules) >= 2, "the modules were merged"
for path in pathlib.Path("shop").glob("*.py"):
    for node in ast.walk(ast.parse(path.read_text())):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            for inner in ast.walk(node):
                assert not isinstance(inner, (ast.Import, ast.ImportFrom)), f"import inside a function in {path}"
from shop.orders import Order
o = Order("guest"); o.add("pen", 1)
assert o.total() == 2.0
print("ok")
