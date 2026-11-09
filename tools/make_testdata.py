"""Builds a realistic test scenario: a 'laptop' folder and an 'external HDD' folder."""
import os, sys, shutil, random, time
from PIL import Image, ImageDraw

root = sys.argv[1]
shutil.rmtree(root, ignore_errors=True)
pc = os.path.join(root, "Laptop", "Slike")
hdd = os.path.join(root, "HDD")

def photo(path, seed, date=None, gps=None, model=None, size=(1600, 1200)):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    random.seed(seed)
    img = Image.new("RGB", size, (random.randrange(256), random.randrange(256), random.randrange(256)))
    d = ImageDraw.Draw(img)
    for _ in range(25):
        x, y = random.randrange(size[0]), random.randrange(size[1])
        d.ellipse([x, y, x + random.randrange(50, 400), y + random.randrange(50, 400)],
                  fill=(random.randrange(256), random.randrange(256), random.randrange(256)))
    d.text((40, 40), f"#{seed}", fill=(255, 255, 255))
    ex = Image.Exif()
    if model:
        ex[0x010F] = model.split()[0]; ex[0x0110] = model
    if date:
        ifd = ex.get_ifd(0x8769); ifd[0x9003] = date
    if gps:
        lat, lon = gps
        g = ex.get_ifd(0x8825)
        def dms(v):
            v = abs(v); dd = int(v); m = int((v - dd) * 60); s = round(((v - dd) * 60 - m) * 60, 2)
            return (float(dd), float(m), float(s))
        g[1] = "N" if lat >= 0 else "S"; g[2] = dms(lat)
        g[3] = "E" if lon >= 0 else "W"; g[4] = dms(lon)
    img.save(path, "JPEG", quality=85, exif=ex)

BG = (44.8125, 20.4612); BUDVA = (42.2864, 18.8400); ROMA = (41.9028, 12.4964)
# laptop
photo(f"{pc}/Leto 2024/IMG_0001.jpg", 1, "2024:07:15 10:00:00", BUDVA, "Apple iPhone 13")
photo(f"{pc}/Leto 2024/IMG_0002.jpg", 2, "2024:07:16 11:00:00", BUDVA, "Apple iPhone 13")
photo(f"{pc}/Leto 2024/IMG_0003.jpg", 3, "2024:07:17 12:00:00", BUDVA, "Apple iPhone 13")
photo(f"{pc}/Rim/DSC_100.jpg", 4, "2023:05:02 09:30:00", ROMA, "samsung SM-A525F")
photo(f"{pc}/Rim/DSC_101.jpg", 5, "2023:05:03 09:30:00", ROMA, "samsung SM-A525F")
photo(f"{pc}/Viber/IMG-20240101-WA0001.jpg", 6)            # date only in the name
photo(f"{pc}/Rodjendan/slika.jpg", 7, "2022:11:20 18:00:00", BG)
photo(f"{pc}/Rodjendan/slika kopija.jpg", 7, "2022:11:20 18:00:00", BG)  # duplicate inside source
photo(f"{pc}/Filteri/Vivid/IMG_0002.jpg", 2, "2024:07:16 11:00:00", BUDVA, "Apple iPhone 13")  # same as Leto/IMG_0002
photo(f"{pc}/Privatno/tajna.jpg", 8)
with open(f"{pc}/Leto 2024/video.mp4", "wb") as f: f.write(os.urandom(300_000))
with open(f"{pc}/Leto 2024/Thumbs.db", "wb") as f: f.write(b"junk")
# HDD: already has some photos, under other names / folders
photo(f"{hdd}/Stare slike/2024/budva1.jpg", 1, "2024:07:15 10:00:00", BUDVA, "Apple iPhone 13")   # = IMG_0001
photo(f"{hdd}/Rim/DSC_100.jpg", 4, "2023:05:02 09:30:00", ROMA, "samsung SM-A525F")                 # same place
photo(f"{hdd}/Leto 2024/IMG_0003.jpg", 33, "2024:07:17 12:00:00", BUDVA, "Apple iPhone 13")         # same name, different
photo(f"{hdd}/Rim/DSC_101.jpg", 55, "2023:05:03 09:30:00", ROMA, "samsung SM-A525F")                # older version
old = time.time() - 86400 * 30
os.utime(f"{hdd}/Rim/DSC_101.jpg", (old, old))
newer_hdd = time.time() + 3600
os.utime(f"{hdd}/Leto 2024/IMG_0003.jpg", (newer_hdd, newer_hdd))  # disk copy is newer -> conflict
photo(f"{hdd}/Dupli/a.jpg", 9); photo(f"{hdd}/Dupli/b (1).jpg", 9); photo(f"{hdd}/Dupli/c - Copy.jpg", 9)
print("ok", root)
