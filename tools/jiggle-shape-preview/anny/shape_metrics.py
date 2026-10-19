"""Geometry-based checks: dimensions and cross sections, independent of recipes."""
from itertools import product
import numpy as np
from scipy.spatial import ConvexHull
from anny_data import NEUTRAL
from validate_profile import interpolate_grid


def section_girth(points, faces, face_weights, origin, direction):
    direction = direction / np.linalg.norm(direction)
    reference = np.array([0.,-1.,0.])
    if abs(reference@direction) > .95: reference = np.array([1.,0.,0.])
    x = np.cross(reference,direction); x /= np.linalg.norm(x)
    basis = np.array([x,np.cross(direction,x)])
    tri = points[faces[face_weights > .55]]
    distances = (tri-origin) @ direction
    intersects = (distances.min(axis=1) < 0) & (distances.max(axis=1) > 0)
    tri, distances = tri[intersects], distances[intersects]
    intersections = []
    for i,j in [(0,1),(1,2),(2,0)]:
        mask = distances[:,i]*distances[:,j] < 0
        t = distances[mask,i]/(distances[mask,i]-distances[mask,j])
        intersections.extend(tri[mask,i]+(tri[mask,j]-tri[mask,i])*t[:,None])
    intersections = np.asarray(intersections)
    assert len(intersections)>=3, "missing anatomical cross section"
    planar = (intersections-origin)@basis.T
    return float(ConvexHull(planar).area)


def check_semantics(transfer, knots, samples, lods):
    t = transfer; points, weights = t.layouts[0]; bones = len(t.target_names)
    faces,offset=[],0
    for s in lods[0]: faces.extend(s.triangles+offset); offset+=len(s.positions)
    faces=np.asarray(faces)
    groups=dict(waist=("lower_lumbar","upper_lumbar",.5),chest=("thoracic","cervical",.35),upper_arm=("lhumerus","lradius",.4),thigh=("lfemurYZ","ltibia",.4))
    region_weights={}
    for label,(bone,end,fraction) in groups.items():
        gi=t.group_ids[bone]
        w=weights[:,gi].copy()
        if label in ("waist","chest"):
            w=sum(weights[:,t.group_ids[g]] for g in ["pelvis","lower_lumbar","upper_lumbar","thoracic"])
        region_weights[label]=w[faces].mean(axis=1)
    neutral=interpolate_grid(knots,samples,NEUTRAL)
    def geometry(c):
        delta=interpolate_grid(knots,samples,c)-neutral
        return t.target_heads+delta[:bones],points+delta[bones:bones+len(points)]
    def measure(c):
        pivots,shaped=geometry(c); result=dict(height=float(np.ptp(shaped[:,2])))
        for label,(bone,end,fraction) in groups.items():
            a,b=pivots[t.ids[bone]],pivots[t.ids[end]]
            result[label]=section_girth(shaped,faces,region_weights[label],a+(b-a)*fraction,b-a)
        return result
    measurements=[];height_error=0.;pivot_error=0.;waist_growth=[]
    # Muscle/weight interactions at young, adult and elder proportion endpoints.
    for age,proportions,muscle in product([0,.5,1],[0,.5,1],[0,.5,1]):
        baseline=[.5,.5,proportions,age]
        p0,q0=geometry(baseline)
        girths=[]
        for weight in [0,.25,.5,.75,1]:
            c=[muscle,weight,proportions,age];p,q=geometry(c);m=measure(c)
            height_error=max(height_error,abs(m["height"]-np.ptp(q0[:,2])))
            pivot_error=max(pivot_error,float(abs(p-p0).max()))
            girths.append(m["waist"])
        assert np.all(np.diff(girths)>0), ("weight must increase waist girth",age,proportions,muscle,girths)
        waist_growth.append(girths[-1]/girths[0]-1)
    assert height_error < .002, ("tissue controls changed height",height_error)
    assert pivot_error < 2e-5, ("tissue controls changed bone pivots",pivot_error)
    # The source's muscle control also reduces abdominal fat; compare muscle
    # bulk at high weight, where the authored target adds muscle mass.
    lean=measure([.5,1,.5,.5]);bulk=measure([1,1,.5,.5])
    assert bulk["upper_arm"]>lean["upper_arm"] and bulk["chest"]>lean["chest"], ("missing arm/chest muscle bulk",lean,bulk)
    child=measure([.5,.5,.5,0]);adult=measure(NEUTRAL);elder=measure([.5,.5,.5,1])
    assert .6 < child["height"]/adult["height"] < .9, ("child stature",child,adult)
    assert .9 < elder["height"]/adult["height"] < 1.05, ("elder stature",elder,adult)
    for label,c in [("low_weight",[.5,0,.5,.5]),("neutral",NEUTRAL),("high_weight",[.5,1,.5,.5]),("low_muscle",[0,.5,.5,.5]),("high_muscle",[1,.5,.5,.5]),("bulky",[1,1,.5,.5]),("child",[.5,.5,.5,0]),("elder",[.5,.5,.5,1])]:
        measurements.append(dict(case=label,controls=list(c),**measure(c)))
    return dict(tissue_recipes=135,max_tissue_height_change=height_error,max_tissue_pivot_change=pivot_error,
                min_weight_waist_gain=float(min(waist_growth)),max_weight_waist_gain=float(max(waist_growth)),
                high_weight_muscle_arm_gain=bulk["upper_arm"]/lean["upper_arm"]-1,
                high_weight_muscle_thigh_gain=bulk["thigh"]/lean["thigh"]-1,measurements=measurements)
