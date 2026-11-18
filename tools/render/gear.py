"""Ronnie 3000 amp and cab skins, rendered in Blender.

Run: Blender -b -P gear.py -- <skin> <out_dir> [samples]
Amps: highgain, modern-us, british, chime, clean, bass, default-clean, default-crunch, default-hi-gain
Cabs: cab-412-modern, cab-412-vintage, cab-212-chime, cab-112-clean, cab-810-bass, cab-default
Units: 1 BU = 1 cm. Front view with a slight high angle, shared lighting.
"""
import math
import os
import sys

import bmesh
import bpy
from mathutils import Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import finish, knob_rig  # noqa: E402

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
SKIN = argv[0] if argv else "highgain"
OUT = argv[1] if len(argv) > 1 else os.path.join(os.getcwd(), SKIN)
SAMPLES = int(argv[2]) if len(argv) > 2 else 256
TITLE = argv[3] if len(argv) > 3 else None

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.unit_settings.system = "METRIC"
scene.unit_settings.scale_length = 0.01

# ================================================================ material builders
def new_mat(name):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    return m, m.node_tree, m.node_tree.nodes["Principled BSDF"]


def setp(p, **kw):
    names = {"color": "Base Color", "metallic": "Metallic", "rough": "Roughness", "coat": "Coat Weight",
             "coat_rough": "Coat Roughness", "aniso": "Anisotropic", "sheen": "Sheen Weight"}
    for k, v in kw.items():
        key = names[k]
        if key in p.inputs:
            p.inputs[key].default_value = (*v, 1) if k == "color" else v


def principled(name, color, **kw):
    m, nt, p = new_mat(name)
    setp(p, color=color, **kw)
    return m


def emission_only(name, color, strength):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    nt = m.node_tree
    for n in list(nt.nodes):
        nt.nodes.remove(n)
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    em = nt.nodes.new("ShaderNodeEmission")
    em.inputs["Color"].default_value = (*color, 1)
    em.inputs["Strength"].default_value = strength
    nt.links.new(em.outputs["Emission"], out.inputs["Surface"])
    return m


def obj_coords(nt, scale=(1, 1, 1), rot_z=0.0):
    tc = nt.nodes.new("ShaderNodeTexCoord")
    mp = nt.nodes.new("ShaderNodeMapping")
    mp.inputs["Scale"].default_value = scale
    mp.inputs["Rotation"].default_value = (0, 0, rot_z)
    nt.links.new(tc.outputs["Object"], mp.inputs["Vector"])
    return mp.outputs["Vector"]


def tolex(name, color, grain):
    m, nt, p = new_mat(name)
    setp(p, color=color, rough=0.68, coat=0.04, coat_rough=0.5)
    vec = obj_coords(nt)
    bump_in = None
    if grain == "levant":
        vor = nt.nodes.new("ShaderNodeTexVoronoi")
        vor.inputs["Scale"].default_value = 1.6
        nt.links.new(vec, vor.inputs["Vector"])
        b1 = nt.nodes.new("ShaderNodeBump")
        b1.inputs["Strength"].default_value = 0.55
        b1.inputs["Distance"].default_value = 0.08
        nt.links.new(vor.outputs["Distance"], b1.inputs["Height"])
        bump_in = b1
    noise = nt.nodes.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = 7
    noise.inputs["Detail"].default_value = 12
    nt.links.new(vec, noise.inputs["Vector"])
    b2 = nt.nodes.new("ShaderNodeBump")
    b2.inputs["Strength"].default_value = 0.35
    b2.inputs["Distance"].default_value = 0.03
    nt.links.new(noise.outputs["Fac"], b2.inputs["Height"])
    if bump_in:
        nt.links.new(bump_in.outputs["Normal"], b2.inputs["Normal"])
    nt.links.new(b2.outputs["Normal"], p.inputs["Normal"])
    return m


def brushed(name, color, metallic=1.0, rough=0.26, aniso=0.7):
    m, nt, p = new_mat(name)
    setp(p, color=color, metallic=metallic, rough=rough, aniso=aniso)
    vec = obj_coords(nt, scale=(0.05, 40, 0.05))
    noise = nt.nodes.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = 3
    noise.inputs["Detail"].default_value = 10
    nt.links.new(vec, noise.inputs["Vector"])
    b = nt.nodes.new("ShaderNodeBump")
    b.inputs["Strength"].default_value = 0.12
    b.inputs["Distance"].default_value = 0.01
    nt.links.new(noise.outputs["Fac"], b.inputs["Height"])
    nt.links.new(b.outputs["Normal"], p.inputs["Normal"])
    return m


def cloth(name, kind, a, b, see_through=0.0):
    """Grille cloth. Pattern computed in object space (cm). see_through mixes in transparency."""
    m, nt, p = new_mat(name)
    setp(p, color=a, rough=0.9, sheen=0.08)
    if kind == "diamond":
        vec = obj_coords(nt, rot_z=math.radians(45))
        tex = nt.nodes.new("ShaderNodeTexBrick")
        tex.offset = 0.0
        tex.inputs["Scale"].default_value = 0.85
        tex.inputs["Mortar Size"].default_value = 0.06
        tex.inputs["Brick Width"].default_value = 1.0
        tex.inputs["Row Height"].default_value = 1.0
        tex.inputs["Color1"].default_value = (*a, 1)
        tex.inputs["Color2"].default_value = (*a, 1)
        tex.inputs["Mortar"].default_value = (*b, 1)
        nt.links.new(vec, tex.inputs["Vector"])
        col, fac = tex.outputs["Color"], tex.outputs["Fac"]
    elif kind == "basket":
        vec = obj_coords(nt)
        tex = nt.nodes.new("ShaderNodeTexChecker")
        tex.inputs["Scale"].default_value = 3.2
        tex.inputs["Color1"].default_value = (*a, 1)
        tex.inputs["Color2"].default_value = (*b, 1)
        nt.links.new(vec, tex.inputs["Vector"])
        col, fac = tex.outputs["Color"], tex.outputs["Fac"]
    elif kind == "sparkle":
        vec = obj_coords(nt)
        wave = nt.nodes.new("ShaderNodeTexWave")
        wave.wave_type = "BANDS"
        wave.bands_direction = "Y"
        wave.inputs["Scale"].default_value = 18
        nt.links.new(vec, wave.inputs["Vector"])
        mix = nt.nodes.new("ShaderNodeMix")
        mix.data_type = "RGBA"
        mix.inputs["A"].default_value = (*a, 1)
        mix.inputs["B"].default_value = (*b, 1)
        nt.links.new(wave.outputs["Fac"], mix.inputs["Factor"])
        setp(p, metallic=0.35, rough=0.5)
        col, fac = mix.outputs["Result"], wave.outputs["Fac"]
    elif kind == "grid":
        vec = obj_coords(nt, scale=(1.4, 1.4, 1.4))
        sep = nt.nodes.new("ShaderNodeSeparateXYZ")
        nt.links.new(vec, sep.inputs["Vector"])
        parts = []
        for axis in ("X", "Y"):
            fr = nt.nodes.new("ShaderNodeMath")
            fr.operation = "FRACT"
            nt.links.new(sep.outputs[axis], fr.inputs[0])
            sub = nt.nodes.new("ShaderNodeMath")
            sub.operation = "SUBTRACT"
            sub.inputs[1].default_value = 0.5
            nt.links.new(fr.outputs[0], sub.inputs[0])
            parts.append(sub)
        comb = nt.nodes.new("ShaderNodeCombineXYZ")
        nt.links.new(parts[0].outputs[0], comb.inputs["X"])
        nt.links.new(parts[1].outputs[0], comb.inputs["Y"])
        ln = nt.nodes.new("ShaderNodeVectorMath")
        ln.operation = "LENGTH"
        nt.links.new(comb.outputs["Vector"], ln.inputs[0])
        lt = nt.nodes.new("ShaderNodeMath")
        lt.operation = "LESS_THAN"
        lt.inputs[1].default_value = 0.3
        nt.links.new(ln.outputs["Value"], lt.inputs[0])
        mix = nt.nodes.new("ShaderNodeMix")
        mix.data_type = "RGBA"
        mix.inputs["A"].default_value = (*a, 1)
        mix.inputs["B"].default_value = (*b, 1)
        nt.links.new(lt.outputs[0], mix.inputs["Factor"])
        setp(p, metallic=0.6, rough=0.45)
        col, fac = mix.outputs["Result"], lt.outputs[0]
    else:  # "mesh" / "cloth": fine square weave
        vec = obj_coords(nt)
        tex = nt.nodes.new("ShaderNodeTexChecker")
        tex.inputs["Scale"].default_value = 5.0 if kind == "mesh" else 7.0
        tex.inputs["Color1"].default_value = (*a, 1)
        tex.inputs["Color2"].default_value = (*b, 1)
        nt.links.new(vec, tex.inputs["Vector"])
        col, fac = tex.outputs["Color"], tex.outputs["Fac"]
    nt.links.new(col, p.inputs["Base Color"])
    bump = nt.nodes.new("ShaderNodeBump")
    bump.inputs["Strength"].default_value = 0.6
    bump.inputs["Distance"].default_value = 0.04
    nt.links.new(fac, bump.inputs["Height"])
    nt.links.new(bump.outputs["Normal"], p.inputs["Normal"])
    if see_through > 0:
        out = nt.nodes["Material Output"]
        mixs = nt.nodes.new("ShaderNodeMixShader")
        tr = nt.nodes.new("ShaderNodeBsdfTransparent")
        mixs.inputs["Fac"].default_value = see_through
        nt.links.new(p.outputs["BSDF"], mixs.inputs[1])
        nt.links.new(tr.outputs["BSDF"], mixs.inputs[2])
        nt.links.new(mixs.outputs["Shader"], out.inputs["Surface"])
    return m


M_CHROME = principled("chrome", (0.9, 0.9, 0.9), metallic=1.0, rough=0.08)
M_NICKEL = principled("nickel", (0.75, 0.74, 0.72), metallic=1.0, rough=0.22)
M_BLACK = principled("black plastic", (0.015, 0.015, 0.015), rough=0.4, coat=0.5, coat_rough=0.15)
M_RUBBER = principled("rubber", (0.02, 0.02, 0.02), rough=0.8)
M_HOLE = principled("hole", (0.0, 0.0, 0.0), rough=1.0)
M_BAFFLE = principled("baffle", (0.008, 0.008, 0.008), rough=0.9)
M_CONE = principled("speaker cone", (0.03, 0.028, 0.025), rough=0.85)
M_DUST = principled("dust cap", (0.05, 0.05, 0.05), rough=0.5)
M_LEATHER = principled("leather", (0.02, 0.018, 0.016), rough=0.5, coat=0.2, coat_rough=0.3)
KNOB_M = {
    "white": principled("knob white", (0.82, 0.82, 0.8), rough=0.3, coat=0.6, coat_rough=0.1),
    "cream": principled("knob cream", (0.74, 0.66, 0.5), rough=0.32, coat=0.5, coat_rough=0.12),
    "black": M_BLACK,
    "gold": principled("knob gold", (0.85, 0.62, 0.25), metallic=1.0, rough=0.22),
    "chrome": M_CHROME,
}

# ================================================================ geometry helpers
def assign(obj, mat):
    obj.data.materials.clear()
    obj.data.materials.append(mat)


def smooth(obj):
    for poly in obj.data.polygons:
        poly.use_smooth = True


def bevel(obj, width, segments=6):
    mod = obj.modifiers.new("bevel", "BEVEL")
    mod.width = width
    mod.segments = segments
    mod.limit_method = "ANGLE"
    try:
        mod.harden_normals = True
    except AttributeError:
        pass
    smooth(obj)


def box(name, size, loc, mat, bev=0.0, seg=6, parent=None):
    bpy.ops.mesh.primitive_cube_add(size=1, location=loc)
    o = bpy.context.active_object
    o.name = name
    o.scale = size
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    if bev:
        bevel(o, bev, seg)
    assign(o, mat)
    if parent:
        o.parent = parent
    return o


def cyl(name, r, depth, loc, mat, verts=64, rot=(0, 0, 0), bev=0.0, parent=None):
    bpy.ops.mesh.primitive_cylinder_add(vertices=verts, radius=r, depth=depth, location=loc, rotation=rot)
    o = bpy.context.active_object
    o.name = name
    if bev:
        bevel(o, bev, 4)
    assign(o, mat)
    if parent:
        o.parent = parent
    return o


def load_font(paths):
    for p in paths:
        if os.path.exists(p):
            try:
                return bpy.data.fonts.load(p)
            except Exception:
                pass
    return None


FONT = load_font(["/System/Library/Fonts/Supplemental/Futura.ttc", "/System/Library/Fonts/Avenir Next.ttc"])
SCRIPT_FONT = load_font(["/System/Library/Fonts/Supplemental/Futura.ttc"])


def text(body, size, loc, mat, parent=None, extrude=0.0, shear=0.0, bold=0.0, font=FONT, align="CENTER"):
    curve = bpy.data.curves.new(type="FONT", name=body)
    curve.body = body
    curve.size = size
    curve.align_x = align
    curve.align_y = "CENTER"
    curve.extrude = extrude
    curve.shear = shear
    curve.offset = bold
    if font:
        curve.font = font
    o = bpy.data.objects.new("txt " + body, curve)
    bpy.context.collection.objects.link(o)
    o.location = loc
    o.data.materials.append(mat)
    if parent:
        o.parent = parent
    return o


def front_frame(D):
    """Empty whose local XY is the front face (x right, y up), local +Z pointing at the camera."""
    f = bpy.data.objects.new("front", None)
    bpy.context.collection.objects.link(f)
    f.location = (0, -D / 2, 0)
    f.rotation_euler = (math.radians(90), 0, 0)
    return f


def knob(F, x, y, z, r, style, ink, label, value=0.55, label_size=0.75):
    """A knob on the front plane F at local (x, y), protruding toward the camera from depth z."""
    base_mat = {"chicken-white": KNOB_M["white"], "chicken-cream": KNOB_M["cream"], "chicken-black": M_BLACK}.get(style, M_BLACK)
    ang = math.radians(225 - 270 * value)
    if style.startswith("chicken"):
        cyl("knob base", r, 1.0, (x, y, z + 0.5), base_mat, verts=64, bev=0.18, parent=F)
        bpy.ops.mesh.primitive_cube_add(size=1, location=(0, 0, 0))
        ptr = bpy.context.active_object
        ptr.scale = (r * 0.55, r * 1.9, 0.9)
        bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
        me = ptr.data
        bm = bmesh.new()
        bm.from_mesh(me)
        for v in bm.verts:
            if v.co.y > 0:  # taper the tip
                v.co.x *= 0.35
        bm.to_mesh(me)
        bm.free()
        bevel(ptr, 0.18, 4)
        assign(ptr, base_mat)
        ptr.location = (x + math.cos(ang) * r * 0.25, y + math.sin(ang) * r * 0.25, z + 1.35)
        ptr.rotation_euler = (0, 0, ang - math.pi / 2)
        ptr.parent = F
        line = box("knob line", (0.14, r * 1.2, 0.05), (x + math.cos(ang) * r * 0.55, y + math.sin(ang) * r * 0.55, z + 1.82), M_BLACK if style != "chicken-black" else KNOB_M["white"], parent=F)
        line.rotation_euler = (0, 0, ang - math.pi / 2)
        movers = [(ptr, r * 0.25, None), (line, r * 0.55, None)]
    else:
        # knurled skirt + cap, cap colour by style
        bpy.ops.mesh.primitive_cylinder_add(vertices=72, radius=r, depth=1.1, location=(0, 0, 0))
        sk = bpy.context.active_object
        bm = bmesh.new()
        bm.from_mesh(sk.data)
        step = 2 * math.pi / 72
        for v in bm.verts:
            if round(math.atan2(v.co.y, v.co.x) / step) % 2:
                v.co.x *= 0.93
                v.co.y *= 0.93
        bm.to_mesh(sk.data)
        bm.free()
        bevel(sk, 0.05, 2)
        assign(sk, M_BLACK)
        sk.location = (x, y, z + 0.55)
        sk.parent = F
        cap_mat = {"gold-cap": KNOB_M["gold"], "skirt": KNOB_M["chrome"], "knurl-black": M_BLACK, "knurl-chrome": M_CHROME}.get(style, M_BLACK)
        cap_r = r * (0.62 if style in ("gold-cap", "skirt") else 0.85)
        cyl("knob cap", cap_r, 0.5, (x, y, z + 1.3), cap_mat, verts=64, bev=0.14, parent=F)
        lm = KNOB_M["white"] if cap_mat is M_BLACK else M_BLACK
        line = box("knob line", (0.12, cap_r * 0.85, 0.05), (x + math.cos(ang) * cap_r * 0.48, y + math.sin(ang) * cap_r * 0.48, z + 1.57), lm, parent=F)
        line.rotation_euler = (0, 0, ang - math.pi / 2)
        movers = [(line, cap_r * 0.48, None)]
    knob_rig.add(label, (x, y), lambda: F.matrix_world @ Vector((x, y, z + 1.6)), r, None, movers)
    text(label, label_size, (x, y + r + 1.0, z + 0.02), ink, parent=F)


def jewel(F, x, y, z, color):
    bpy.ops.mesh.primitive_torus_add(major_radius=1.0, minor_radius=0.32, location=(x, y, z + 0.25))
    t = bpy.context.active_object
    smooth(t)
    assign(t, M_CHROME)
    t.parent = F
    bpy.ops.mesh.primitive_ico_sphere_add(subdivisions=1, radius=0.8, location=(x, y, z + 0.3))
    j = bpy.context.active_object
    assign(j, emission_only("jewel", color, 3.0))
    j.parent = F
    glow = bpy.data.lights.new("jewel glow", "POINT")
    glow.energy = 60
    glow.color = color
    glow.shadow_soft_size = 0.4
    g = bpy.data.objects.new("jewel glow", glow)
    g.location = (x, y, z + 2.0)
    g.parent = F
    bpy.context.collection.objects.link(g)


def jack(F, x, y, z, ink, label):
    cyl("jack nut", 0.85, 0.35, (x, y, z + 0.17), M_NICKEL, verts=6, bev=0.05, parent=F)
    cyl("jack ring", 0.55, 0.4, (x, y, z + 0.4), M_NICKEL, verts=48, bev=0.05, parent=F)
    cyl("jack hole", 0.32, 0.1, (x, y, z + 0.58), M_HOLE, verts=32, parent=F)
    text(label, 0.6, (x, y + 1.7, z + 0.02), ink, parent=F)


def led(F, x, y, z, color, on):
    cyl("led bezel", 0.42, 0.25, (x, y, z + 0.12), M_CHROME, verts=32, bev=0.06, parent=F)
    bpy.ops.mesh.primitive_uv_sphere_add(radius=0.3, location=(x, y, z + 0.3))
    s = bpy.context.active_object
    smooth(s)
    assign(s, emission_only("led", color, 2.5) if on else principled("led off", tuple(c * 0.12 for c in color), rough=0.2))
    s.parent = F


def rocker(F, x, y, z, body_color, ink, label):
    box("rocker frame", (1.4, 2.4, 0.3), (x, y, z + 0.15), M_BLACK, bev=0.12, seg=3, parent=F)
    r = box("rocker", (1.0, 1.9, 0.5), (x, y, z + 0.45), principled("rocker", body_color, rough=0.35, coat=0.4), bev=0.15, seg=3, parent=F)
    r.rotation_euler = (math.radians(12), 0, 0)
    text(label, 0.5, (x, y + 2.0, z + 0.02), ink, parent=F)


def bat_toggle(F, x, y, z, ink, label):
    cyl("toggle nut", 0.55, 0.3, (x, y, z + 0.15), M_NICKEL, verts=6, bev=0.04, parent=F)
    lever = cyl("toggle bat", 0.13, 1.5, (x, y + 0.35, z + 0.9), M_CHROME, verts=24, rot=(math.radians(-25), 0, 0), parent=F)
    text(label, 0.55, (x, y + 1.9, z + 0.02), ink, parent=F)


def badge(F, kind, title, x, y, z, ink):
    if kind in ("silver", "steel"):
        w = 0.95 * len(title) + 2.6
        mat = M_CHROME if kind == "silver" else brushed("badge steel", (0.75, 0.76, 0.78))
        box("badge plate", (w, 2.6, 0.25), (x, y, z + 0.12), mat, bev=0.2, seg=4, parent=F)
        text(title.upper() if kind == "steel" else title, 1.35 if kind == "silver" else 1.1, (x, y - 0.05, z + 0.27),
             principled("badge ink", (0.03, 0.03, 0.03), rough=0.4), parent=F, extrude=0.03, shear=0.2 if kind == "silver" else 0.0, bold=0.03)
    elif kind == "script":
        text(title, 3.0, (x, y, z + 0.1), ink, parent=F, extrude=0.18, shear=0.25, bold=0.05)
    else:
        text(title.upper(), 1.0, (x, y, z + 0.05), ink, parent=F, extrude=0.02)


def corners(W, D, H):
    for sx in (-1, 1):
        for sz in (0, 1):
            x = sx * (W / 2 - 0.4)
            z = 0.4 if sz == 0 else H - 0.4
            box("corner", (3.2, 3.2, 3.2), (x, -D / 2 + 1.0, z), M_CHROME, bev=1.0, seg=6)
            cyl("corner screw", 0.25, 0.2, (x - sx * 0.9, -D / 2 - 0.62, z + (0.9 if sz == 0 else -0.9)), M_NICKEL, verts=16, rot=(math.radians(90), 0, 0))


def strap_handle(W, H, D, width=26.0, mat=None):
    mat = mat or M_LEATHER
    for side in (-1, 1):
        box("handle cap", (3.6, 4.2, 1.2), (side * width / 2, 0, H + 0.5), M_CHROME, bev=0.4, seg=4)
    curve = bpy.data.curves.new("strap", "CURVE")
    curve.dimensions = "3D"
    sp = curve.splines.new("BEZIER")
    sp.bezier_points.add(2)
    pts = [(-width / 2, 0, H + 1.0), (0, 0, H + 4.2), (width / 2, 0, H + 1.0)]
    for bp, co in zip(sp.bezier_points, pts):
        bp.co = co
        bp.handle_left_type = bp.handle_right_type = "AUTO"
    curve.bevel_depth = 0.75
    curve.bevel_resolution = 4
    o = bpy.data.objects.new("strap", curve)
    o.scale = (1, 2.4, 1)
    bpy.context.collection.objects.link(o)
    o.data.materials.append(mat)


def bar_handle(W, H, width=34.0):
    curve = bpy.data.curves.new("bar", "CURVE")
    curve.dimensions = "3D"
    sp = curve.splines.new("POLY")
    pts = [(-width / 2, 0, H), (-width / 2, 0, H + 3.5), (width / 2, 0, H + 3.5), (width / 2, 0, H)]
    sp.points.add(len(pts) - 1)
    for p, co in zip(sp.points, pts):
        p.co = (*co, 1)
    curve.bevel_depth = 0.6
    curve.bevel_resolution = 6
    o = bpy.data.objects.new("bar handle", curve)
    bpy.context.collection.objects.link(o)
    o.data.materials.append(M_CHROME)
    for side in (-1, 1):
        box("bar mount", (3.0, 3.0, 1.0), (side * width / 2, 0, H + 0.4), M_BLACK, bev=0.3, seg=3)


def feet(W, D, inset=6.0):
    for sx in (-1, 1):
        for sy in (-1, 1):
            cyl("foot", 1.6, 1.2, (sx * (W / 2 - inset), sy * (D / 2 - 4), 0.6), M_RUBBER, verts=32, bev=0.3)


def casters(W, D):
    for sx in (-1, 1):
        for sy in (-1, 1):
            x, y = sx * (W / 2 - 5), sy * (D / 2 - 5)
            box("caster plate", (5, 5, 0.6), (x, y, 5.6), M_NICKEL, bev=0.2, seg=2)
            cyl("caster fork", 0.8, 2.4, (x, y, 4.3), M_NICKEL, verts=24)
            cyl("caster wheel", 2.4, 1.6, (x, y - 1.0, 2.4), M_RUBBER, verts=48, rot=(0, math.radians(90), 0), bev=0.4)


def speakers(F, centers, r, z_back, depth=4.0):
    """Baffle plus real speaker cones behind the grille cloth."""
    for (x, y) in centers:
        bpy.ops.mesh.primitive_cone_add(vertices=64, radius1=r, radius2=r * 0.22, depth=depth, end_fill_type="NOTHING", location=(x, y, z_back + depth / 2), rotation=(math.radians(180), 0, 0))
        c = bpy.context.active_object
        smooth(c)
        assign(c, M_CONE)
        c.parent = F
        bpy.ops.mesh.primitive_torus_add(major_radius=r * 0.98, minor_radius=r * 0.06, location=(x, y, z_back + depth))
        s = bpy.context.active_object
        smooth(s)
        assign(s, M_CONE)
        s.parent = F
        bpy.ops.mesh.primitive_uv_sphere_add(radius=r * 0.24, location=(x, y, z_back + 0.6))
        d = bpy.context.active_object
        d.scale = (1, 1, 0.45)
        smooth(d)
        assign(d, M_DUST)
        d.parent = F

# ================================================================ skins
AMPS = {
    "highgain": dict(form="head", W=66, H=27, D=26, tolex=(0.012, 0.012, 0.012), grain="fine", grille=("mesh", (0.01, 0.01, 0.01), (0.05, 0.05, 0.05)), piping=None,
                     panel=dict(color=(0.03, 0.03, 0.03), metallic=0.6, rough=0.32), panel_top=False, ink=(0.85, 0.85, 0.83), knob="skirt",
                     pilot=(1.0, 0.03, 0.0), leds=[(0.1, 1.0, 0.3), (1.0, 0.05, 0.0)], rockers=None, jacks=["INPUT"], badge=("silver", "HG 150", "right"), handle="strap", corners=True),
    "modern-us": dict(form="head", W=68, H=27, D=27, tolex=(0.012, 0.012, 0.012), grain="fine", grille=("mesh", (0.008, 0.008, 0.008), (0.04, 0.04, 0.04)), piping=None,
                      panel=dict(color=(0.72, 0.73, 0.75), metallic=1.0, rough=0.24), panel_top=False, ink=(0.03, 0.03, 0.03), knob="knurl-black",
                      pilot=(0.1, 1.0, 0.25), leds=[(0.1, 1.0, 0.3), (1.0, 0.5, 0.0), (1.0, 0.05, 0.0)], rockers=None, jacks=["INPUT"], badge=("steel", "Modern 100", "center"), handle="bar", corners=True),
    "british": dict(form="head", W=74, H=29, D=24, tolex=(0.014, 0.014, 0.014), grain="levant", grille=("cloth", (0.012, 0.012, 0.011), (0.05, 0.048, 0.042)), piping=(0.85, 0.82, 0.74),
                    panel=dict(color=(0.85, 0.6, 0.22), metallic=1.0, rough=0.26), panel_top=True, ink=(0.03, 0.02, 0.01), knob="gold-cap",
                    pilot=(1.0, 0.03, 0.0), leds=None, rockers=[(0.02, 0.02, 0.02), (0.02, 0.02, 0.02)], jacks=["HIGH", "LOW"], badge=("plain", "Brit 100", "left"), handle="strap", corners=True),
    "chime": dict(form="combo", W=70, H=56, D=26, tolex=(0.012, 0.012, 0.012), grain="fine", grille=("diamond", (0.16, 0.09, 0.045), (0.62, 0.5, 0.3)), piping=(0.8, 0.74, 0.6),
                  panel=dict(color=(0.75, 0.38, 0.17), metallic=1.0, rough=0.28), panel_top=True, ink=(0.03, 0.015, 0.01), knob="chicken-cream",
                  pilot=(1.0, 0.03, 0.0), leds=None, rockers=None, jacks=["IN"], badge=("plain", "Chime 30", "topcenter"), badge_ink=(0.93, 0.88, 0.76), handle="strap", corners=True, speakers=[(-15, -6), (15, -6)]),
    "clean": dict(form="combo", W=66, H=50, D=25, tolex=(0.012, 0.012, 0.012), grain="fine", grille=("sparkle", (0.1, 0.105, 0.115), (0.62, 0.63, 0.66)), piping=(0.8, 0.81, 0.82),
                  panel=dict(color=(0.02, 0.02, 0.02), metallic=0.3, rough=0.35), panel_top=True, ink=(0.88, 0.88, 0.86), knob="skirt",
                  pilot=(1.0, 0.03, 0.0), leds=None, rockers=None, jacks=["1", "2"], badge=("silver", "Clean 85", "topleft"), handle="strap", corners=True, speakers=[(-14, -5), (14, -5)]),
    "bass": dict(form="head", W=66, H=34, D=28, tolex=(0.014, 0.014, 0.016), grain="fine", grille=("cloth", (0.01, 0.01, 0.011), (0.045, 0.045, 0.05)), piping=(0.7, 0.72, 0.74),
                 panel=dict(color=(0.74, 0.75, 0.77), metallic=1.0, rough=0.24), panel_top=True, ink=(0.03, 0.03, 0.03), knob="knurl-black",
                 pilot=(1.0, 0.03, 0.0), leds=None, rockers=[(0.85, 0.85, 0.83), (0.85, 0.85, 0.83)], jacks=["INPUT"], badge=("steel", "Bass 300", "bottomright"), handle="bar", corners=True),
}
for k, grille_a, grille_b, title in (("default-clean", (0.5, 0.5, 0.48), (0.38, 0.38, 0.36), "bedroom-clean"),
                                    ("default-crunch", (0.24, 0.15, 0.08), (0.14, 0.09, 0.05), "capture-03"),
                                    ("default-hi-gain", (0.02, 0.02, 0.02), (0.06, 0.06, 0.065), "my-rig-lead")):
    AMPS[k] = dict(form="head", W=64, H=27, D=25, tolex=(0.05, 0.05, 0.055), grain="fine", grille=("cloth", grille_a, grille_b), piping=(0.2, 0.2, 0.21),
                   panel=dict(color=(0.08, 0.08, 0.085), metallic=0.7, rough=0.3), panel_top=False, ink=(0.86, 0.86, 0.84), knob="chicken-white",
                   pilot=(1.0, 0.25, 0.0), leds=None, rockers=None, jacks=["INPUT"], badge=("plain", title, "bottomleft"), handle="strap", corners=False, mark=True)

CABS = {
    "cab-412-modern": dict(W=76, H=76, D=36, tolex=(0.012, 0.012, 0.012), grain="fine", grille=("mesh", (0.01, 0.01, 0.01), (0.05, 0.05, 0.05)), piping=None, see=0.55,
                           spk=[(-17, 17), (17, 17), (-17, -17), (17, -17)], r=14.5, badge=("silver", "Modern 4x12", "bottomright"), handle=None, corners=True),
    "cab-412-vintage": dict(W=76, H=76, D=36, tolex=(0.014, 0.014, 0.014), grain="levant", grille=("basket", (0.06, 0.052, 0.04), (0.34, 0.3, 0.22)), piping=(0.86, 0.83, 0.75), see=0.12,
                            spk=[(-17, 17), (17, 17), (-17, -17), (17, -17)], r=14.5, badge=("plain", "Vintage 4x12", "topcenter"), handle=None, corners=True),
    "cab-212-chime": dict(W=74, H=50, D=28, tolex=(0.012, 0.012, 0.012), grain="fine", grille=("diamond", (0.16, 0.09, 0.045), (0.62, 0.5, 0.3)), piping=(0.8, 0.74, 0.6), see=0.12,
                          spk=[(-16, -1), (16, -1)], r=14.5, badge=("plain", "Chime 2x12", "topcenter"), badge_ink=(0.93, 0.88, 0.76), handle="strap", corners=True),
    "cab-112-clean": dict(W=52, H=48, D=26, tolex=(0.012, 0.012, 0.012), grain="fine", grille=("sparkle", (0.1, 0.105, 0.115), (0.62, 0.63, 0.66)), piping=(0.8, 0.81, 0.82), see=0.3,
                          spk=[(0, -1)], r=15.5, badge=("silver", "Clean 1x12", "topleft"), handle="strap", corners=True),
    "cab-810-bass": dict(W=62, H=120, D=40, tolex=(0.014, 0.014, 0.016), grain="fine", grille=("grid", (0.04, 0.04, 0.045), (0.0, 0.0, 0.0)), piping=None, see=0.3,
                         spk=[(sx * 13, 41 - i * 25) for i in range(4) for sx in (-1, 1)], r=10.5, badge=("steel", "Bass 8x10", "topcenter"), handle=None, corners=False, casters=True),
    "cab-default": dict(W=72, H=48, D=28, tolex=(0.05, 0.05, 0.055), grain="fine", grille=("cloth", (0.06, 0.06, 0.065), (0.11, 0.11, 0.115)), piping=(0.2, 0.2, 0.21), see=0.18,
                        spk=[(-16, -1), (16, -1)], r=14.5, badge=("plain", "my-ir-mix", "bottomleft"), handle="strap", corners=False),
}

# ================================================================ build
is_cab = SKIN in CABS
cfg = CABS[SKIN] if is_cab else AMPS[SKIN]
W, H, D = cfg["W"], cfg["H"], cfg["D"]
lift = 6.6 if cfg.get("casters") else 1.2
INK = principled("ink", cfg.get("ink", (0.86, 0.86, 0.84)), rough=0.5)

body = box("tolex body", (W, D, H), (0, 0, lift + H / 2), tolex("tolex", cfg["tolex"], cfg["grain"]), bev=1.3, seg=8)
F = front_frame(D)
F.location.z = lift
title = TITLE or cfg["badge"][1]


def cut_front(x0, x1, y0, y1, depth=7.0):
    """Boolean an opening into the tolex body: the front frame keeps its border, the inside is recessed."""
    bpy.ops.mesh.primitive_cube_add(size=1, location=((x0 + x1) / 2, -D / 2 + depth / 2 - 1.0, lift + (y0 + y1) / 2))
    cutter = bpy.context.active_object
    cutter.name = "front cutter"
    cutter.scale = (x1 - x0, depth + 2.0, y1 - y0)
    cutter.hide_render = True
    cutter.display_type = "WIRE"
    mod = body.modifiers.new("opening", "BOOLEAN")
    mod.operation = "DIFFERENCE"
    mod.object = cutter
    try:
        mod.solver = "EXACT"
    except Exception:
        pass



def grille_area(x0, x1, y0, y1, speakers_at=None, r=14.5, see=0.0):
    gw, gh = x1 - x0, y1 - y0
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    # recess: dark baffle behind, cloth panel slightly inset
    box("baffle", (gw, gh, 0.6), (cx, cy, -6.5), M_BAFFLE, parent=F)
    if speakers_at:
        speakers(F, [(cx + sx, cy + sy) for sx, sy in speakers_at], r, -6.2)
    kind, a, b = cfg["grille"]
    box("grille cloth", (gw, gh, 0.15), (cx, cy, -0.35), cloth("grille", kind, a, b, see_through=see), parent=F)
    if cfg.get("piping"):
        pm = principled("piping", cfg["piping"], rough=0.4, coat=0.3)
        for (sx, sy, lx, ly) in ((gw + 0.7, 0.7, cx, y1), (gw + 0.7, 0.7, cx, y0), (0.7, gh + 0.7, x0, cy), (0.7, gh + 0.7, x1, cy)):
            box("piping", (sx, sy, 0.7), (lx, ly, 0.05), pm, bev=0.3, seg=4, parent=F)
    return cx, cy, gw, gh


def badge_pos(where, x0, x1, y0, y1):
    return {"right": (x1 - 9, y1 - 4), "left": (x0 + 14, (y0 + y1) / 2), "center": ((x0 + x1) / 2, y1 - 4),
            "topcenter": ((x0 + x1) / 2, y1 - 4), "topleft": (x0 + 10, y1 - 4), "bottomright": (x1 - 11, y0 + 4),
            "bottomleft": (x0 + 10, y0 + 3.5)}[where]


if is_cab:
    m = 3.4
    x0, x1, y0, y1 = -W / 2 + m, W / 2 - m, m, H - m
    cut_front(x0, x1, y0, y1)
    cx, cy, gw, gh = grille_area(x0, x1, y0, y1, [(sx, sy) for sx, sy in cfg["spk"]], cfg["r"], cfg["see"])
    kind, title_txt, where = cfg["badge"]
    bx, by = badge_pos(where, x0, x1, y0, y1)
    badge(F, kind, title, bx, by, 0.1, principled("script ink", cfg.get("badge_ink", (0.88, 0.85, 0.76)), rough=0.4) if kind == "script" or cfg.get("badge_ink") else INK)
else:
    m = 2.6
    panel_h = 8.0 if cfg["form"] == "head" else 8.5
    x0, x1 = -W / 2 + m, W / 2 - m
    if cfg["panel_top"]:
        py0, py1 = H - m - panel_h, H - m
        gy0, gy1 = m, py0 - 1.4
    else:
        py0, py1 = m, m + panel_h
        gy0, gy1 = py1 + 1.4, H - m
    cut_front(x0, x1, m, H - m)
    tol = tolex("tolex strip", cfg["tolex"], cfg["grain"])
    gap_y = (py0 - 0.7) if cfg["panel_top"] else (py1 + 0.7)
    box("gap strip", (x1 - x0, 1.4, 7.0), (0, gap_y, -3.5), tol, parent=F)
    box("panel block", (x1 - x0, py1 - py0, 7.0), (0, (py0 + py1) / 2, -3.75), M_BAFFLE, parent=F)
    spk = cfg.get("speakers")
    grille_area(x0, x1, gy0, gy1, spk, 12.5, 0.14 if spk else 0.0)
    pc = cfg["panel"]
    pmat = brushed("panel", pc["color"], metallic=pc["metallic"], rough=pc["rough"])
    pcy = (py0 + py1) / 2
    box("control panel", (x1 - x0, panel_h, 0.5), (0, pcy, 0.0), pmat, bev=0.15, seg=3, parent=F)
    if cfg["form"] == "head" and cfg["panel"]["metallic"] > 0.9 and SKIN in ("modern-us", "bass"):
        for sx in (-1, 1):
            for sy in (-1, 1):
                cyl("panel screw", 0.32, 0.2, (sx * (x1 - x0) / 2 - sx * 1.0, pcy + sy * (panel_h / 2 - 1.0), 0.3), M_NICKEL, verts=24, parent=F)
    ink = INK
    z = 0.25
    x = x0 + 4.5
    knob_y = pcy - 0.8
    for label in cfg["jacks"]:
        jack(F, x, knob_y, z, ink, label)
        x += 4.2
    x += 2.5
    kr = 1.55 if cfg["form"] == "head" else 1.45
    for label, val in (("GAIN IN", 0.62), ("OUTPUT", 0.5), ("MIX", 1.0)):
        knob(F, x, knob_y, z, kr, cfg["knob"], ink, label, val)
        x += 6.2
    bat_toggle(F, x - 1.0, knob_y, z, ink, "NORM")
    x += 4.5
    if cfg.get("leds"):
        for i, c in enumerate(cfg["leds"]):
            led(F, x + i * 2.0, knob_y, z, c, i == 0)
            text(str(i + 1), 0.5, (x + i * 2.0, knob_y - 1.3, z + 0.02), ink, parent=F)
        text("CAPTURE", 0.5, (x + (len(cfg["leds"]) - 1), knob_y + 1.6, z + 0.02), ink, parent=F)
    xr = x1 - 4.0
    if cfg.get("mark"):
        text("ronnie", 1.7, (xr - 3.0, knob_y, z + 0.05), ink, parent=F, extrude=0.03, shear=0.2, bold=0.03)
        xr -= 9.5
    jewel(F, xr, knob_y, z, cfg["pilot"])
    if cfg.get("rockers"):
        rx = xr - 4.0
        for label, col in zip(("POWER", "STANDBY"), cfg["rockers"]):
            rocker(F, rx, knob_y, z, col, ink, label)
            rx -= 3.6
    kind, _, where = cfg["badge"]
    bx, by = badge_pos(where, x0, x1, gy0, gy1)
    badge(F, kind, title, bx, by, 0.1, principled("badge ink", (0.92, 0.9, 0.84), rough=0.4) if kind == "script" else
          principled("plain ink", cfg.get("badge_ink") or ((0.85, 0.85, 0.83) if sum(cfg["grille"][1]) < 1.0 else (0.04, 0.04, 0.04)), rough=0.5))

if cfg.get("corners"):
    corners(W, D, H + lift * 0)
if cfg.get("handle") == "strap":
    strap_handle(W, H + lift, D, width=min(30.0, W * 0.42))
elif cfg.get("handle") == "bar":
    bar_handle(W, H + lift, width=min(36.0, W * 0.5))
if cfg.get("casters"):
    casters(W, D)
else:
    feet(W, D)

# corners were placed relative to z=0; shift them up by lift
for o in bpy.data.objects:
    if o.name.startswith("corner"):
        o.location.z += lift

# ================================================================ ground, lights, camera, render
bpy.ops.mesh.primitive_plane_add(size=2000, location=(0, 0, 0))
ground = bpy.context.active_object
ground.is_shadow_catcher = True
assign(ground, principled("ground", (0.8, 0.8, 0.8), rough=0.8))

world = bpy.data.worlds.new("world")
scene.world = world
world.use_nodes = True
wn = world.node_tree
bg = wn.nodes["Background"]
hdri = os.path.join(bpy.utils.system_resource("DATAFILES"), "studiolights", "world", "interior.exr")
if os.path.exists(hdri):
    env = wn.nodes.new("ShaderNodeTexEnvironment")
    env.image = bpy.data.images.load(hdri)
    mapping = wn.nodes.new("ShaderNodeMapping")
    coord = wn.nodes.new("ShaderNodeTexCoord")
    wn.links.new(coord.outputs["Generated"], mapping.inputs["Vector"])
    mapping.inputs["Rotation"].default_value = (0, 0, math.radians(320))
    wn.links.new(mapping.outputs["Vector"], env.inputs["Vector"])
    wn.links.new(env.outputs["Color"], bg.inputs["Color"])
bg.inputs["Strength"].default_value = 0.22

SCALE = max(W, H) / 12.0  # lights were tuned for a 12 cm pedal; scale distances and power with gear size


def area(name, loc, size, energy, color=(1, 1, 1), target=None, shadow=True):
    l = bpy.data.lights.new(name, "AREA")
    l.shape = "RECTANGLE"
    l.size, l.size_y = size
    l.energy = energy
    l.color = color
    if not shadow:
        try:
            l.use_shadow = False
        except Exception:
            pass
    o = bpy.data.objects.new(name, l)
    o.location = loc
    bpy.context.collection.objects.link(o)
    tgt = Vector(target or (0, 0, lift + H / 2))
    o.rotation_euler = (tgt - Vector(loc)).to_track_quat("-Z", "Y").to_euler()
    return o


cz = lift + H / 2
area("key softbox", (-W * 0.9, -W * 1.4, cz + H * 1.6 + 30), (W * 1.4, W * 1.0), 15000 * SCALE ** 2 * 0.38, (1.0, 0.97, 0.93))
area("fill", (W * 1.3, -W * 1.2, cz + 10), (W, W), 3500 * SCALE ** 2 * 0.3, (0.93, 0.96, 1.0), shadow=False)
area("rim", (0, W * 1.6, cz + H * 1.4 + 20), (W * 1.6, W * 0.4), 5000 * SCALE ** 2 * 0.4)

cam_data = bpy.data.cameras.new("cam")
cam_data.lens = 85
cam_data.sensor_fit = "AUTO"
cam = bpy.data.objects.new("cam", cam_data)
bpy.context.collection.objects.link(cam)
frame = max(W * 1.18, (H + lift + 6) * 1.18 * (1.0 if W >= H else 1.0))
dist = frame * 85 / 36
elev = math.radians(9)
target = Vector((0, 0, (lift + H + 4) / 2))
cam.location = (0, -dist * math.cos(elev), target.z + dist * math.sin(elev))
cam.rotation_euler = (target - cam.location).to_track_quat("-Z", "Y").to_euler()
scene.camera = cam

scene.render.engine = "CYCLES"
prefs = bpy.context.preferences.addons["cycles"].preferences
try:
    prefs.compute_device_type = "METAL"
    prefs.get_devices()
    for dev in prefs.devices:
        dev.use = True
    scene.cycles.device = "GPU"
except Exception as exc:
    print("GPU unavailable:", exc)
scene.cycles.samples = SAMPLES
scene.cycles.use_denoising = True
scene.render.film_transparent = True
if W >= H:
    scene.render.resolution_x, scene.render.resolution_y = 1600, int(1600 * (H + lift + 14) * 1.18 / frame)
else:
    scene.render.resolution_x, scene.render.resolution_y = int(1500 * W * 1.18 / frame) + 200, 1500
scene.render.image_settings.file_format = "PNG"
scene.render.image_settings.color_mode = "RGBA"
try:
    scene.view_settings.view_transform = "AgX"
    scene.view_settings.look = "AgX - Medium High Contrast"
except TypeError:
    pass
finish(scene, OUT, SKIN, SAMPLES)
