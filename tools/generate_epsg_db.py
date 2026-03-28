#!/usr/bin/env python3
"""Generate epsg.json — a curated list of common EPSG CRS codes."""

import json

# Curated list: (code, name, area)
ENTRIES = [
    # ── Geographic (lat/lon) ────────────────────────────────────────────────
    (4326,  "WGS 84",                          "World"),
    (4269,  "NAD83",                           "North America"),
    (4267,  "NAD27",                           "North America"),
    (4979,  "WGS 84 (3D)",                     "World"),
    (4978,  "WGS 84 (ECEF)",                   "World"),
    (4155,  "ETRS89",                          "Europe"),
    (4896,  "ETRS89 (3D)",                     "Europe"),

    # ── US National ─────────────────────────────────────────────────────────
    (5070,  "NAD83 / Conus Albers",            "United States"),
    (3857,  "WGS 84 / Pseudo-Mercator",        "World"),
    (2163,  "US National Atlas Equal Area",    "United States"),

    # ── UTM Zones — NAD83 (continental US) ─────────────────────────────────
    (26903, "NAD83 / UTM zone 3N",             "United States"),
    (26904, "NAD83 / UTM zone 4N",             "United States"),
    (26905, "NAD83 / UTM zone 5N",             "United States"),
    (26906, "NAD83 / UTM zone 6N",             "United States"),
    (26907, "NAD83 / UTM zone 7N",             "United States"),
    (26908, "NAD83 / UTM zone 8N",             "United States"),
    (26909, "NAD83 / UTM zone 9N",             "United States"),
    (26910, "NAD83 / UTM zone 10N",            "United States"),
    (26911, "NAD83 / UTM zone 11N",           "United States"),
    (26912, "NAD83 / UTM zone 12N",           "United States"),
    (26913, "NAD83 / UTM zone 13N",           "United States"),
    (26914, "NAD83 / UTM zone 14N",           "United States"),
    (26915, "NAD83 / UTM zone 15N",           "United States"),
    (26916, "NAD83 / UTM zone 16N",           "United States"),
    (26917, "NAD83 / UTM zone 17N",           "United States"),
    (26918, "NAD83 / UTM zone 18N",           "United States"),
    (26919, "NAD83 / UTM zone 19N",           "United States"),

    # ── UTM Zones — WGS84 Northern ─────────────────────────────────────────
    (32601, "WGS 84 / UTM zone 1N",            "World"),
    (32602, "WGS 84 / UTM zone 2N",            "World"),
    (32603, "WGS 84 / UTM zone 3N",            "World"),
    (32604, "WGS 84 / UTM zone 4N",            "World"),
    (32605, "WGS 84 / UTM zone 5N",            "World"),
    (32606, "WGS 84 / UTM zone 6N",            "World"),
    (32607, "WGS 84 / UTM zone 7N",            "World"),
    (32608, "WGS 84 / UTM zone 8N",            "World"),
    (32609, "WGS 84 / UTM zone 9N",            "World"),
    (32610, "WGS 84 / UTM zone 10N",           "World"),
    (32611, "WGS 84 / UTM zone 11N",           "World"),
    (32612, "WGS 84 / UTM zone 12N",           "World"),
    (32613, "WGS 84 / UTM zone 13N",           "World"),
    (32614, "WGS 84 / UTM zone 14N",           "World"),
    (32615, "WGS 84 / UTM zone 15N",           "World"),
    (32616, "WGS 84 / UTM zone 16N",           "World"),
    (32617, "WGS 84 / UTM zone 17N",           "World"),
    (32618, "WGS 84 / UTM zone 18N",           "World"),
    (32619, "WGS 84 / UTM zone 19N",           "World"),
    (32620, "WGS 84 / UTM zone 20N",           "World"),
    (32621, "WGS 84 / UTM zone 21N",           "World"),
    (32622, "WGS 84 / UTM zone 22N",           "World"),
    (32623, "WGS 84 / UTM zone 23N",           "World"),
    (32624, "WGS 84 / UTM zone 24N",           "World"),
    (32625, "WGS 84 / UTM zone 25N",           "World"),
    (32626, "WGS 84 / UTM zone 26N",           "World"),
    (32627, "WGS 84 / UTM zone 27N",           "World"),
    (32628, "WGS 84 / UTM zone 28N",           "World"),
    (32629, "WGS 84 / UTM zone 29N",           "World"),
    (32630, "WGS 84 / UTM zone 30N",           "World"),
    (32631, "WGS 84 / UTM zone 31N",           "World"),
    (32632, "WGS 84 / UTM zone 32N",           "World"),
    (32633, "WGS 84 / UTM zone 33N",           "World"),
    (32634, "WGS 84 / UTM zone 34N",           "World"),
    (32635, "WGS 84 / UTM zone 35N",           "World"),
    (32636, "WGS 84 / UTM zone 36N",           "World"),
    (32637, "WGS 84 / UTM zone 37N",           "World"),
    (32638, "WGS 84 / UTM zone 38N",           "World"),
    (32639, "WGS 84 / UTM zone 39N",           "World"),
    (32640, "WGS 84 / UTM zone 40N",           "World"),
    (32641, "WGS 84 / UTM zone 41N",           "World"),
    (32642, "WGS 84 / UTM zone 42N",           "World"),
    (32643, "WGS 84 / UTM zone 43N",           "World"),
    (32644, "WGS 84 / UTM zone 44N",           "World"),
    (32645, "WGS 84 / UTM zone 45N",           "World"),
    (32646, "WGS 84 / UTM zone 46N",           "World"),
    (32647, "WGS 84 / UTM zone 47N",           "World"),
    (32648, "WGS 84 / UTM zone 48N",           "World"),
    (32649, "WGS 84 / UTM zone 49N",           "World"),
    (32650, "WGS 84 / UTM zone 50N",           "World"),
    (32651, "WGS 84 / UTM zone 51N",           "World"),
    (32652, "WGS 84 / UTM zone 52N",           "World"),
    (32653, "WGS 84 / UTM zone 53N",           "World"),
    (32654, "WGS 84 / UTM zone 54N",           "World"),
    (32655, "WGS 84 / UTM zone 55N",           "World"),
    (32656, "WGS 84 / UTM zone 56N",           "World"),
    (32657, "WGS 84 / UTM zone 57N",           "World"),
    (32658, "WGS 84 / UTM zone 58N",           "World"),
    (32659, "WGS 84 / UTM zone 59N",           "World"),
    (32660, "WGS 84 / UTM zone 60N",           "World"),

    # ── UTM Zones — WGS84 Southern ─────────────────────────────────────────
    (32701, "WGS 84 / UTM zone 1S",            "World"),
    (32702, "WGS 84 / UTM zone 2S",            "World"),
    (32703, "WGS 84 / UTM zone 3S",            "World"),
    (32704, "WGS 84 / UTM zone 4S",            "World"),
    (32705, "WGS 84 / UTM zone 5S",            "World"),
    (32706, "WGS 84 / UTM zone 6S",            "World"),
    (32707, "WGS 84 / UTM zone 7S",            "World"),
    (32708, "WGS 84 / UTM zone 8S",            "World"),
    (32709, "WGS 84 / UTM zone 9S",            "World"),
    (32710, "WGS 84 / UTM zone 10S",           "World"),
    (32711, "WGS 84 / UTM zone 11S",           "World"),
    (32712, "WGS 84 / UTM zone 12S",           "World"),
    (32713, "WGS 84 / UTM zone 13S",           "World"),
    (32714, "WGS 84 / UTM zone 14S",           "World"),
    (32715, "WGS 84 / UTM zone 15S",           "World"),
    (32716, "WGS 84 / UTM zone 16S",           "World"),
    (32717, "WGS 84 / UTM zone 17S",           "World"),
    (32718, "WGS 84 / UTM zone 18S",           "World"),
    (32719, "WGS 84 / UTM zone 19S",           "World"),
    (32720, "WGS 84 / UTM zone 20S",           "World"),
    (32721, "WGS 84 / UTM zone 21S",           "World"),
    (32722, "WGS 84 / UTM zone 22S",           "World"),
    (32723, "WGS 84 / UTM zone 23S",           "World"),
    (32724, "WGS 84 / UTM zone 24S",           "World"),
    (32725, "WGS 84 / UTM zone 25S",           "World"),
    (32726, "WGS 84 / UTM zone 26S",           "World"),
    (32727, "WGS 84 / UTM zone 27S",           "World"),
    (32728, "WGS 84 / UTM zone 28S",           "World"),
    (32729, "WGS 84 / UTM zone 29S",           "World"),
    (32730, "WGS 84 / UTM zone 30S",           "World"),
    (32731, "WGS 84 / UTM zone 31S",           "World"),
    (32732, "WGS 84 / UTM zone 32S",           "World"),
    (32733, "WGS 84 / UTM zone 33S",           "World"),
    (32734, "WGS 84 / UTM zone 34S",           "World"),
    (32735, "WGS 84 / UTM zone 35S",           "World"),
    (32736, "WGS 84 / UTM zone 36S",           "World"),
    (32737, "WGS 84 / UTM zone 37S",           "World"),
    (32738, "WGS 84 / UTM zone 38S",           "World"),
    (32739, "WGS 84 / UTM zone 39S",           "World"),
    (32740, "WGS 84 / UTM zone 40S",           "World"),
    (32741, "WGS 84 / UTM zone 41S",           "World"),
    (32742, "WGS 84 / UTM zone 42S",           "World"),
    (32743, "WGS 84 / UTM zone 43S",           "World"),
    (32744, "WGS 84 / UTM zone 44S",           "World"),
    (32745, "WGS 84 / UTM zone 45S",           "World"),
    (32746, "WGS 84 / UTM zone 46S",           "World"),
    (32747, "WGS 84 / UTM zone 47S",           "World"),
    (32748, "WGS 84 / UTM zone 48S",           "World"),
    (32749, "WGS 84 / UTM zone 49S",           "World"),
    (32750, "WGS 84 / UTM zone 50S",           "World"),
    (32751, "WGS 84 / UTM zone 51S",           "World"),
    (32752, "WGS 84 / UTM zone 52S",           "World"),
    (32753, "WGS 84 / UTM zone 53S",           "World"),
    (32754, "WGS 84 / UTM zone 54S",           "World"),
    (32755, "WGS 84 / UTM zone 55S",           "World"),
    (32756, "WGS 84 / UTM zone 56S",           "World"),
    (32757, "WGS 84 / UTM zone 57S",           "World"),
    (32758, "WGS 84 / UTM zone 58S",           "World"),
    (32759, "WGS 84 / UTM zone 59S",           "World"),
    (32760, "WGS 84 / UTM zone 60S",           "World"),

    # ── Alaska ───────────────────────────────────────────────────────────────
    (3338,  "NAD83 / Alaska Albers",           "United States — Alaska"),
    (26931, "NAD83 / Alaska zone 1",          "United States — Alaska"),
    (26932, "NAD83 / Alaska zone 2",          "United States — Alaska"),
    (26933, "NAD83 / Alaska zone 3",          "United States — Alaska"),
    (26934, "NAD83 / Alaska zone 4",          "United States — Alaska"),
    (26935, "NAD83 / Alaska zone 5",          "United States — Alaska"),
    (26936, "NAD83 / Alaska zone 6",          "United States — Alaska"),
    (26937, "NAD83 / Alaska zone 7",          "United States — Alaska"),
    (26938, "NAD83 / Alaska zone 8",          "United States — Alaska"),
    (26939, "NAD83 / Alaska zone 9",          "United States — Alaska"),
    (26940, "NAD83 / Alaska zone 10",         "United States — Alaska"),

    # ── Hawaii & US Territories ─────────────────────────────────────────────
    (26951, "NAD83 / Hawaii zone 1",          "United States — Hawaii"),
    (26952, "NAD83 / Hawaii zone 2",          "United States — Hawaii"),
    (26953, "NAD83 / Hawaii zone 3",          "United States — Hawaii"),
    (26954, "NAD83 / Hawaii zone 4",          "United States — Hawaii"),
    (26955, "NAD83 / Hawaii zone 5",          "United States — Hawaii"),
    (32145, "NAD83(HARN) / Hawaii zone 3",   "United States — Hawaii"),
    (32146, "NAD83(HARN) / Hawaii zone 4",   "United States — Hawaii"),
    (32147, "NAD83(HARN) / Hawaii zone 5",   "United States — Hawaii"),
    (32604, "WGS 84 / UTM zone 4N",           "United States — Hawaii"),
    (32605, "WGS 84 / UTM zone 5N",           "United States — Hawaii"),

    # ── US State Plane NAD83 — major zones ──────────────────────────────────
    # Alabama
    (2201,  "NAD83 / Alabama East",           "United States — Alabama"),
    (2202,  "NAD83 / Alabama West",           "United States — Alabama"),
    # Alaska (covered above)
    # Arizona
    (3021,  "NAD83 / Arizona East",           "United States — Arizona"),
    (3022,  "NAD83 / Arizona Central",        "United States — Arizona"),
    (3023,  "NAD83 / Arizona West",           "United States — Arizona"),
    # Arkansas
    (3401,  "NAD83 / Arkansas North",         "United States — Arkansas"),
    (3402,  "NAD83 / Arkansas South",         "United States — Arkansas"),
    # California
    (2225,  "NAD83 / California zone 1",      "United States — California"),
    (2226,  "NAD83 / California zone 2",      "United States — California"),
    (2227,  "NAD83 / California zone 3",      "United States — California"),
    (2228,  "NAD83 / California zone 4",      "United States — California"),
    (2229,  "NAD83 / California zone 5",      "United States — California"),
    (2230,  "NAD83 / California zone 6",      "United States — California"),
    (2231,  "NAD83 / California zone 7",      "United States — California"),
    (2232,  "NAD83 / California zone 8",      "United States — California"),
    (2233,  "NAD83 / California zone 9",      "United States — California"),
    # Colorado
    (2876,  "NAD83 / Colorado North",         "United States — Colorado"),
    (2877,  "NAD83 / Colorado Central",       "United States — Colorado"),
    (2878,  "NAD83 / Colorado South",         "United States — Colorado"),
    # Connecticut
    (2235,  "NAD83 / Connecticut",             "United States — Connecticut"),
    (2236,  "NAD83 / Connecticut (ft)",        "United States — Connecticut"),
    # Delaware
    (2249,  "NAD83 / Delaware",               "United States — Delaware"),
    # Florida
    (2239,  "NAD83 / Florida East",           "United States — Florida"),
    (2240,  "NAD83 / Florida North",          "United States — Florida"),
    (2241,  "NAD83 / Florida West",            "United States — Florida"),
    # Georgia
    (2244,  "NAD83 / Georgia East",           "United States — Georgia"),
    (2245,  "NAD83 / Georgia West",           "United States — Georgia"),
    # Idaho
    (2551,  "NAD83 / Idaho East",              "United States — Idaho"),
    (2552,  "NAD83 / Idaho Central",         "United States — Idaho"),
    (2553,  "NAD83 / Idaho West",             "United States — Idaho"),
    # Illinois
    (2403,  "NAD83 / Illinois East",          "United States — Illinois"),
    (2404,  "NAD83 / Illinois West",          "United States — Illinois"),
    # Indiana
    (2255,  "NAD83 / Indiana East",           "United States — Indiana"),
    (2256,  "NAD83 / Indiana West",           "United States — Indiana"),
    # Iowa
    (2277,  "NAD83 / Iowa North",             "United States — Iowa"),
    (2278,  "NAD83 / Iowa South",             "United States — Iowa"),
    # Kansas
    (2819,  "NAD83 / Kansas North",           "United States — Kansas"),
    (2820,  "NAD83 / Kansas South",           "United States — Kansas"),
    # Kentucky
    (2285,  "NAD83 / Kentucky North",         "United States — Kentucky"),
    (2286,  "NAD83 / Kentucky South",         "United States — Kentucky"),
    # Louisiana
    (2292,  "NAD83 / Louisiana North",        "United States — Louisiana"),
    (2293,  "NAD83 / Louisiana South",       "United States — Louisiana"),
    (2291,  "NAD83 / Louisiana Offshore",     "United States — Louisiana"),
    # Maryland
    (2289,  "NAD83 / Maryland",               "United States — Maryland"),
    # Massachusetts
    (2248,  "NAD83 / Massachusetts Mainland", "United States — Massachusetts"),
    (2252,  "NAD83 / Massachusetts Island",   "United States — Massachusetts"),
    # Michigan
    (2258,  "NAD83 / Michigan North",         "United States — Michigan"),
    (2259,  "NAD83 / Michigan Central",       "United States — Michigan"),
    (2260,  "NAD83 / Michigan South",         "United States — Michigan"),
    # Minnesota
    (2261,  "NAD83 / Minnesota North",        "United States — Minnesota"),
    (2262,  "NAD83 / Minnesota Central",      "United States — Minnesota"),
    (2263,  "NAD83 / Minnesota South",        "United States — Minnesota"),
    # Mississippi
    (2254,  "NAD83 / Mississippi East",      "United States — Mississippi"),
    (2253,  "NAD83 / Mississippi West",      "United States — Mississippi"),
    # Missouri
    (2401,  "NAD83 / Missouri East",          "United States — Missouri"),
    (2402,  "NAD83 / Missouri Central",      "United States — Missouri"),
    (2405,  "NAD83 / Missouri West",          "United States — Missouri"),
    # Montana
    (2816,  "NAD83 / Montana",               "United States — Montana"),
    # Nebraska
    (3201,  "NAD83 / Nebraska North",         "United States — Nebraska"),
    (3202,  "NAD83 / Nebraska South",         "United States — Nebraska"),
    # Nevada
    (3211,  "NAD83 / Nevada East",            "United States — Nevada"),
    (3212,  "NAD83 / Nevada Central",         "United States — Nevada"),
    (3213,  "NAD83 / Nevada West",            "United States — Nevada"),
    # New Jersey
    (2246,  "NAD83 / New Jersey",             "United States — New Jersey"),
    # New Mexico
    (3214,  "NAD83 / New Mexico East",       "United States — New Mexico"),
    (3215,  "NAD83 / New Mexico Central",    "United States — New Mexico"),
    (3216,  "NAD83 / New Mexico West",       "United States — New Mexico"),
    # New York
    (2264,  "NAD83 / New York East",          "United States — New York"),
    (2265,  "NAD83 / New York Central",       "United States — New York"),
    (2266,  "NAD83 / New York West",          "United States — New York"),
    (2267,  "NAD83 / New York Long Island",  "United States — New York"),
    # North Carolina
    (2268,  "NAD83 / North Carolina",        "United States — North Carolina"),
    # North Dakota
    (2269,  "NAD83 / North Dakota North",    "United States — North Dakota"),
    (2270,  "NAD83 / North Dakota South",    "United States — North Dakota"),
    # Ohio
    (2283,  "NAD83 / Ohio North",             "United States — Ohio"),
    (2284,  "NAD83 / Ohio South",             "United States — Ohio"),
    # Oklahoma
    (2839,  "NAD83 / Oklahoma North",        "United States — Oklahoma"),
    (2840,  "NAD83 / Oklahoma South",        "United States — Oklahoma"),
    # Oregon
    (2837,  "NAD83 / Oregon North",          "United States — Oregon"),
    (2838,  "NAD83 / Oregon South",          "United States — Oregon"),
    # Pennsylvania
    (2834,  "NAD83 / Pennsylvania North",    "United States — Pennsylvania"),
    (2835,  "NAD83 / Pennsylvania South",    "United States — Pennsylvania"),
    # South Carolina
    (2273,  "NAD83 / South Carolina",       "United States — South Carolina"),
    # Tennessee
    (2276,  "NAD83 / Tennessee",             "United States — Tennessee"),
    # Texas
    (2278,  "NAD83 / Texas North",           "United States — Texas"),
    (2279,  "NAD83 / Texas North Central",  "United States — Texas"),
    (2280,  "NAD83 / Texas Central",         "United States — Texas"),
    (2281,  "NAD83 / Texas South Central",  "United States — Texas"),
    (2282,  "NAD83 / Texas South",           "United States — Texas"),
    # Utah
    (2830,  "NAD83 / Utah North",            "United States — Utah"),
    (2831,  "NAD83 / Utah Central",          "United States — Utah"),
    (2832,  "NAD83 / Utah South",            "United States — Utah"),
    # Virginia
    (2289,  "NAD83 / Virginia North",        "United States — Virginia"),
    (2290,  "NAD83 / Virginia South",        "United States — Virginia"),
    # Washington
    (2285,  "NAD83 / Washington North",      "United States — Washington"),
    (2286,  "NAD83 / Washington South",      "United States — Washington"),
    # West Virginia
    (2287,  "NAD83 / West Virginia North",   "United States — West Virginia"),
    (2288,  "NAD83 / West Virginia South",   "United States — West Virginia"),
    # Wisconsin
    (2868,  "NAD83 / Wisconsin North",      "United States — Wisconsin"),
    (2869,  "NAD83 / Wisconsin Central",    "United States — Wisconsin"),
    (2870,  "NAD83 / Wisconsin South",      "United States — Wisconsin"),
    # Wyoming
    (2814,  "NAD83 / Wyoming East",          "United States — Wyoming"),
    (2815,  "NAD83 / Wyoming East Central", "United States — Wyoming"),
    (2816,  "NAD83 / Wyoming West Central", "United States — Wyoming"),
    (2817,  "NAD83 / Wyoming West",          "United States — Wyoming"),

    # ── Canada ───────────────────────────────────────────────────────────────
    (3978,  "NAD83 / Canada Atlas Lambert",   "Canada"),
    (3976,  "NAD83 / Yukon Territorial Boundary", "Canada"),
    (3977,  "NAD83 / Nunavut Territorial Boundary", "Canada"),
    (26907, "NAD83 / UTM zone 7N",            "Canada"),
    (26908, "NAD83 / UTM zone 8N",            "Canada"),
    (26909, "NAD83 / UTM zone 9N",            "Canada"),
    (26910, "NAD83 / UTM zone 10N",           "Canada"),
    (26911, "NAD83 / UTM zone 11N",           "Canada"),
    (26912, "NAD83 / UTM zone 12N",           "Canada"),
    (26913, "NAD83 / UTM zone 13N",           "Canada"),
    (26914, "NAD83 / UTM zone 14N",           "Canada"),
    (26915, "NAD83 / UTM zone 15N",           "Canada"),
    (26916, "NAD83 / UTM zone 16N",           "Canada"),
    (26917, "NAD83 / UTM zone 17N",           "Canada"),
    (26918, "NAD83 / UTM zone 18N",           "Canada"),
    (26919, "NAD83 / UTM zone 19N",           "Canada"),
    (26920, "NAD83 / UTM zone 20N",           "Canada"),
    (26921, "NAD83 / UTM zone 21N",           "Canada"),
    (26922, "NAD83 / UTM zone 22N",           "Canada"),

    # ── Mexico ──────────────────────────────────────────────────────────────
    (6362,  "NAD83 / Mexico Atlas",           "Mexico"),
    (3186,  "NAD83 / UTM zone 11N",           "Mexico"),
    (3187,  "NAD83 / UTM zone 12N",           "Mexico"),
    (3188,  "NAD83 / UTM zone 13N",           "Mexico"),
    (3189,  "NAD83 / UTM zone 14N",           "Mexico"),
    (3190,  "NAD83 / UTM zone 15N",           "Mexico"),
    (3191,  "NAD83 / UTM zone 16N",           "Mexico"),

    # ── Europe ──────────────────────────────────────────────────────────────
    (3035,  "ETRS89 / LAEA Europe",           "Europe"),
    (3034,  "ETRS89 / Lambert Azimuthal Equal Area", "Europe"),
    (25828, "ETRS89 / UTM zone 28N",          "Europe"),
    (25829, "ETRS89 / UTM zone 29N",          "Europe"),
    (25830, "ETRS89 / UTM zone 30N",          "Europe"),
    (25831, "ETRS89 / UTM zone 31N",          "Europe"),
    (25832, "ETRS89 / UTM zone 32N",          "Europe"),
    (25833, "ETRS89 / UTM zone 33N",          "Europe"),
    (25834, "ETRS89 / UTM zone 34N",          "Europe"),
    (25835, "ETRS89 / UTM zone 35N",          "Europe"),
    (25836, "ETRS89 / UTM zone 36N",          "Europe"),
    (25837, "ETRS89 / UTM zone 37N",          "Europe"),
    (3031,  "ETRS89 / ETRS-GK",               "Europe"),
    (3043,  "ETRS89 / LCC Europe",            "Europe"),
    (4258,  "ETRS89",                          "Europe"),

    # ── UK ──────────────────────────────────────────────────────────────────
    (27700, "OSGB 1936 / British National Grid", "United Kingdom"),
    (2157,  "OSGB 1936 / UTM zone 30N",       "United Kingdom"),
    (2991,  "OSGB 1936 / UTM zone 29N",       "United Kingdom"),
    (3855,  "ETRS89 / UTM zone 29N",          "United Kingdom"),
    (3856,  "ETRS89 / UTM zone 30N",          "United Kingdom"),
    (3857,  "ETRS89 / UTM zone 31N",          "United Kingdom"),

    # ── Australia ───────────────────────────────────────────────────────────
    (4283,  "GDA94",                           "Australia"),
    (28348, "GDA94 / MGA zone 48",             "Australia"),
    (28349, "GDA94 / MGA zone 49",             "Australia"),
    (28350, "GDA94 / MGA zone 50",             "Australia"),
    (28351, "GDA94 / MGA zone 51",             "Australia"),
    (28352, "GDA94 / MGA zone 52",             "Australia"),
    (28353, "GDA94 / MGA zone 53",             "Australia"),
    (28354, "GDA94 / MGA zone 54",             "Australia"),
    (28355, "GDA94 / MGA zone 55",             "Australia"),
    (28356, "GDA94 / MGA zone 56",             "Australia"),
    (3577,  "GDA94 / Australian Albers",      "Australia"),

    # ── New Zealand ──────────────────────────────────────────────────────────
    (4167,  "NZGD2000",                        "New Zealand"),
    (2193,  "NZGD2000 / NZTM",                 "New Zealand"),
    (27200, "NZGD49 / New Zealand Map Grid",   "New Zealand"),
    (27205, "NZGD49 / UTM zone 39S",           "New Zealand"),
    (27206, "NZGD49 / UTM zone 40S",           "New Zealand"),

    # ── Japan ───────────────────────────────────────────────────────────────
    (4301,  "JGD2000",                         "Japan"),
    (2452,  "JGD2000 / UTM zone 51N",           "Japan"),
    (2453,  "JGD2000 / UTM zone 52N",           "Japan"),
    (2454,  "JGD2000 / UTM zone 53N",           "Japan"),
    (2455,  "JGD2000 / UTM zone 54N",           "Japan"),
    (2456,  "JGD2000 / UTM zone 55N",           "Japan"),
    (32654, "WGS 84 / UTM zone 54N",           "Japan"),
    (32655, "WGS 84 / UTM zone 55N",           "Japan"),

    # ── South America ────────────────────────────────────────────────────────
    (4615,  "SIRGAS 2000",                     "South America"),
    (29118, "SAD69 / UTM zone 18N",            "South America"),
    (29119, "SAD69 / UTM zone 19N",            "South America"),
    (29120, "SAD69 / UTM zone 20N",            "South America"),
    (29121, "SAD69 / UTM zone 21N",            "South America"),
    (29122, "SAD69 / UTM zone 22N",            "South America"),
    (29177, "SAD69 / UTM zone 17S",            "South America"),
    (29178, "SAD69 / UTM zone 18S",            "South America"),
    (29179, "SAD69 / UTM zone 19S",            "South America"),
    (29180, "SAD69 / UTM zone 20S",            "South America"),
    (29181, "SAD69 / UTM zone 21S",            "South America"),
    (29182, "SAD69 / UTM zone 22S",            "South America"),
    (29183, "SAD69 / UTM zone 23S",            "South America"),
    (29184, "SAD69 / UTM zone 24S",            "South America"),
    (29185, "SAD69 / UTM zone 25S",            "South America"),
    (32717, "WGS 84 / UTM zone 17S",           "South America"),
    (32718, "WGS 84 / UTM zone 18S",           "South America"),
    (32719, "WGS 84 / UTM zone 19S",           "South America"),
    (32720, "WGS 84 / UTM zone 20S",           "South America"),
    (32721, "WGS 84 / UTM zone 21S",           "South America"),
    (32722, "WGS 84 / UTM zone 23S",           "South America"),
    (32723, "WGS 84 / UTM zone 24S",           "South America"),
    (32724, "WGS 84 / UTM zone 25S",           "South America"),

    # ── Africa ────────────────────────────────────────────────────────────────
    (4326,  "WGS 84",                          "Africa"),
    (32628, "WGS 84 / UTM zone 28N",           "Africa"),
    (32629, "WGS 84 / UTM zone 29N",           "Africa"),
    (32630, "WGS 84 / UTM zone 30N",           "Africa"),
    (32631, "WGS 84 / UTM zone 31N",           "Africa"),
    (32632, "WGS 84 / UTM zone 32N",           "Africa"),
    (32633, "WGS 84 / UTM zone 33N",           "Africa"),
    (32634, "WGS 84 / UTM zone 34N",           "Africa"),
    (32635, "WGS 84 / UTM zone 35N",           "Africa"),
    (32636, "WGS 84 / UTM zone 36N",           "Africa"),
    (32637, "WGS 84 / UTM zone 37N",           "Africa"),
    (32735, "WGS 84 / UTM zone 35S",           "Africa"),
    (32736, "WGS 84 / UTM zone 36S",           "Africa"),
    (32737, "WGS 84 / UTM zone 37S",           "Africa"),
    (32738, "WGS 84 / UTM zone 38S",           "Africa"),
    (32739, "WGS 84 / UTM zone 39S",           "Africa"),
    (32740, "WGS 84 / UTM zone 40S",           "Africa"),
    (32741, "WGS 84 / UTM zone 41S",           "Africa"),
    (32742, "WGS 84 / UTM zone 42S",           "Africa"),
    (32743, "WGS 84 / UTM zone 43S",           "Africa"),
    (32744, "WGS 84 / UTM zone 44S",           "Africa"),
    (32745, "WGS 84 / UTM zone 45S",           "Africa"),
    (32746, "WGS 84 / UTM zone 46S",           "Africa"),
    (32747, "WGS 84 / UTM zone 47S",           "Africa"),
    (32748, "WGS 84 / UTM zone 48S",           "Africa"),
    (32749, "WGS 84 / UTM zone 49S",           "Africa"),
    (32750, "WGS 84 / UTM zone 50S",           "Africa"),
    (32751, "WGS 84 / UTM zone 51S",           "Africa"),
    (32752, "WGS 84 / UTM zone 52S",           "Africa"),
    (32753, "WGS 84 / UTM zone 53S",           "Africa"),
    (32754, "WGS 84 / UTM zone 54S",           "Africa"),
    (32755, "WGS 84 / UTM zone 55S",           "Africa"),
    (32756, "WGS 84 / UTM zone 56S",           "Africa"),
]

# Deduplicate by code, preserve insertion order
seen = set()
unique = []
for code, name, area in ENTRIES:
    if code not in seen:
        seen.add(code)
        unique.append({"code": code, "name": name, "area": area})

out_path = "/Users/malkobot/Meridian/meridian-rust/meridian/src/data/epsg.json"
with open(out_path, "w") as f:
    json.dump(unique, f, indent=2)

print(f"Wrote {len(unique)} entries to {out_path}")
