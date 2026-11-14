expected="INFO 3
ERROR 2
WARN 2
DEBUG 1
slowest a4 1500"
actual="$(bash summary.sh app.log)"
if [ "$actual" = "$expected" ]; then echo ok; else echo "got:"; echo "$actual"; exit 1; fi
