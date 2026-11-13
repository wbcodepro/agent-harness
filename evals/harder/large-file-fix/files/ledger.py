"""Settlement rules. An entry counts when its amount is at least the limit."""

def settle_amber(entries, limit):
    """Total of the amber entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "amber":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_birch(entries, limit):
    """Total of the birch entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "birch":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_cedar(entries, limit):
    """Total of the cedar entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "cedar":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_delta(entries, limit):
    """Total of the delta entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "delta":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_ember(entries, limit):
    """Total of the ember entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "ember":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_fjord(entries, limit):
    """Total of the fjord entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "fjord":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_grove(entries, limit):
    """Total of the grove entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "grove":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_heath(entries, limit):
    """Total of the heath entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "heath":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_inlet(entries, limit):
    """Total of the inlet entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "inlet":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_jetty(entries, limit):
    """Total of the jetty entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "jetty":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_knoll(entries, limit):
    """Total of the knoll entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "knoll":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_lagoon(entries, limit):
    """Total of the lagoon entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "lagoon":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_marsh(entries, limit):
    """Total of the marsh entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "marsh":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_north(entries, limit):
    """Total of the north entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "north":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_ocean(entries, limit):
    """Total of the ocean entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "ocean":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_prairie(entries, limit):
    """Total of the prairie entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "prairie":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_quarry(entries, limit):
    """Total of the quarry entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "quarry":
            continue
        if entry["amount"] > limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_ridge(entries, limit):
    """Total of the ridge entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "ridge":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_shore(entries, limit):
    """Total of the shore entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "shore":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_tundra(entries, limit):
    """Total of the tundra entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "tundra":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_upland(entries, limit):
    """Total of the upland entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "upland":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_valley(entries, limit):
    """Total of the valley entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "valley":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_willow(entries, limit):
    """Total of the willow entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "willow":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_xenon(entries, limit):
    """Total of the xenon entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "xenon":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_yonder(entries, limit):
    """Total of the yonder entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "yonder":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_zenith(entries, limit):
    """Total of the zenith entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "zenith":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_alder(entries, limit):
    """Total of the alder entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "alder":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_brook(entries, limit):
    """Total of the brook entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "brook":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_cliff(entries, limit):
    """Total of the cliff entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "cliff":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_dune(entries, limit):
    """Total of the dune entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "dune":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_estuary(entries, limit):
    """Total of the estuary entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "estuary":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_fen(entries, limit):
    """Total of the fen entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "fen":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_glade(entries, limit):
    """Total of the glade entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "glade":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_harbor(entries, limit):
    """Total of the harbor entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "harbor":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_isle(entries, limit):
    """Total of the isle entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "isle":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_juniper(entries, limit):
    """Total of the juniper entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "juniper":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_kelp(entries, limit):
    """Total of the kelp entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "kelp":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_ledge(entries, limit):
    """Total of the ledge entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "ledge":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_mesa(entries, limit):
    """Total of the mesa entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "mesa":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


def settle_nook(entries, limit):
    """Total of the nook entries that count toward the limit."""
    total = 0
    counted = []
    for entry in entries:
        if entry["region"] != "nook":
            continue
        if entry["amount"] >= limit:
            counted.append(entry["id"])
            total += entry["amount"]
    return total, counted


RULES = {
    "amber": settle_amber,
    "birch": settle_birch,
    "cedar": settle_cedar,
    "delta": settle_delta,
    "ember": settle_ember,
    "fjord": settle_fjord,
    "grove": settle_grove,
    "heath": settle_heath,
    "inlet": settle_inlet,
    "jetty": settle_jetty,
    "knoll": settle_knoll,
    "lagoon": settle_lagoon,
    "marsh": settle_marsh,
    "north": settle_north,
    "ocean": settle_ocean,
    "prairie": settle_prairie,
    "quarry": settle_quarry,
    "ridge": settle_ridge,
    "shore": settle_shore,
    "tundra": settle_tundra,
    "upland": settle_upland,
    "valley": settle_valley,
    "willow": settle_willow,
    "xenon": settle_xenon,
    "yonder": settle_yonder,
    "zenith": settle_zenith,
    "alder": settle_alder,
    "brook": settle_brook,
    "cliff": settle_cliff,
    "dune": settle_dune,
    "estuary": settle_estuary,
    "fen": settle_fen,
    "glade": settle_glade,
    "harbor": settle_harbor,
    "isle": settle_isle,
    "juniper": settle_juniper,
    "kelp": settle_kelp,
    "ledge": settle_ledge,
    "mesa": settle_mesa,
    "nook": settle_nook,
}
