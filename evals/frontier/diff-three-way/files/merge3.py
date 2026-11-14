"""Three-way text merge. The behaviour is specified in SPEC.md."""
import sys


def merge3(base, ours, theirs, *, style="merge", ignore_ws=False,
           labels=("ours", "base", "theirs")):
    """Merge `ours` and `theirs`, two edited versions of `base`.

    Returns (text, conflicts). See SPEC.md.
    """
    raise NotImplementedError("merge3 is not written yet")


def main(argv=None):
    """Command line of SPEC.md rules 25 to 29; returns the exit status."""
    raise NotImplementedError("the command line is not written yet")


if __name__ == "__main__":
    sys.exit(main())
