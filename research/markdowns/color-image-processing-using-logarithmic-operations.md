<!-- Converted from color-image-processing-using-logarithmic-operations.pdf — 4 pages -->

## Page 1

## COLOR IMAGE PROCESSING USING LOGARlTHMIC OPERATIONS
### VasilePritra$cu. VasileBuzuloiu
### Image Processingand Analysis Laboratory (LAPI),
## " Politehnica" University of Bucuresti
### E-mail: vpatrascu@hotmail.com, buzuloiu@alpha.imag.pub.ro
experimental results and Section 6outlines the conclusions.
operations: addition (+) and real scalar multiplication (x). With these operations the space ofcolon becomes a real vector space. Then, defining the scalar product (.I.) and the norm /I . 1, we obtaina (loxarithmic) Euclidean space. We show how we can use this model for color image enhancement and we present some
## experimental results.2. THE REAL VECTOR SPACEOF THE
## COLORS
## 1. INTRODUCTION
better suited for analysis by a human or a machine. This area of
### 2.1 Addition
image processing has been and is always an important fieldof the research in which publications of new techniques and methods V v , , v , t E , with v , = ( r l , g , , b , ) , v2=(r2,g,,b,), the sum take a large place in the literature. Many approaches exist
### (contrast manipulation, histogram modification, filtering,..) that vI(+)v, is defined by the following relation:
are well exposed in reference books such as [ I , 61. A great number of enhancement methods exist because each application is specific and needs an adapted method [I]. So, the physical nature of images to be processed is of central importance and the The neutral element for addition is ''0=(O,O,O) . Each need of an adequate image mathematical model appears clearly
### =(r,g,b) E E,
commutative, has a neutral element and each element has an opposite. It results that this operation establishes on E, a
Another approach exists in the general setting of logarithmic operation (-)by:
representations suited for [he imaging processes obtained by transmitted light or for the human visual perception. Jourlin and Pinoli introduced a mathematical framework for this kind of
## '"xinear'' representat$& [2j. With subtraction (-), we will note by (-)v the opposite of v (
work with bounded real sets. The extension of our model to color
### 2.2 Scalarmultiplication
images is natural and we develop it here. We consider the unit
(0.5, 0.5, 0.5) to ( O , O , 0). It ib this new cube, which plays the
## 0-7803-7979-9/03/$17.00 02003IEEE
- 133

---

## Page 2

### =((l+r)* +(I-r)* '(l+g)h.c(l-g)A '(l+b)h+(l-b)'1
(3) The two operations : addition (+) and scalar multiplication (x) establish on E, a real vector spacestructure.
### 2.3 The Euclidean space ofthe colors
Wedefinethrssdarproducr (.\.)E, : E 3 x E 3+R by:
V v , , v 2 ~ E Jwith v l = ( r l , g l , b l ) . v 2 = ( r 2 . g z . b z ) .
(v, I = ~ I , ) . l p ( r , ) + ~ g l ) . ~ g z ) + ~ ( b , ) . l p ( b , )
(4) where l p : ( - l , l j + R and lp(x)=arcth(x)
The norm is defined in the normal way: \(./(a,:E, +LO,-),
$$ Vv =(1,g.b) E E,, $$
/Iv[ I E l =& ( r ) + q * ( g ) + v z(b) ( 5 )
Our color space becomes a three-dimensional Euclidean space.
# 3. THE VECTOR SPACEOFTHE COLOR
# IMAGES
A color image is a function defined on a bi-dimensional compact D from R' taking the values in the color space E,. We note with F(D,EI) the set of color images defined on D. We can extend the operations and the functions from color space E, to color images F(D,E,) in a nahlral way.
# 3.1 Addition
Vf,,f, EF(D,E,) , V X . Y ) E D .
(f, (+)f* )(X.Y) =f t (x,.y)(+)fz (X.Y) ( 6 ) The neutral element is the identical null function. The addition
### (+) is stable, associa!ive, commutative, has a neutral element and
each element has an opposite. As a conclusion, this operation establishes on the set F(D,E,) a commutativegroupstructure.
### 3.2 Scalar multiplication
V h eR,Vf k F(D,E3) , V ( x , y ) e D ,
(h(x)f)(x,y)= h(X)f(X,Y) (7) The two operation;, addition (+) and scalar multiplication (x) establish on F(D,E,) a real vector space s t " m .
### 3.3 The Hilbert space ofthe color images
Let be f and g W O integrable functions from F(D,$). We can define the scalar product by: Vf, , f 2EF(D,E,)
Further on, we will define the norm.
( I + ~ ) ~ - ( l - r ) * (I+g)'-(1-g)* (I+b)'-(l-b)'
Thus, the color images space F(D,E,) becomesa Hilbert space.
# 4. APPLICATIONS IN ENHANCEMENT
# OF COLOR IMAGES
The enhancement algorithms that we will present in this paper consists of afline transformations, t : F ( D , E 3 ) + F ( D , E , )givenbytheequation:
t ( f )=a(x) f (+) P(x) k (10)
where f is the original image and h = t (f ) is the enhanced
determined so that the transformation satisfies some conditions. We shall shldy two sets of conditions. Firstly, we make some preliminary notations. Let be f :D +E, a color image, having
# the scalar components f r :D +E , f, :D +E , f b :D+E .
We consider the following subset of the support D D,, ={(x,y)EDlf~(X,Y)<m(f,(D))}
D,, =( ( x . y ) ~ D l f ~ ( ~ , ~ ) ~ m ( f , ( D ) ) }
D,, = k x , y ) ~ D / f ~ ( x , Y ) < m t f ~ ( D ) ) }
D,, =kx,y)EDIf,(X,y)~m(f,(D))}
D,, ={ ( x . y ) ~ D l f ~ ( x , y ) < m ( f ~ ( D ) ) }
Db2 =((X,Y)E D Ifb(x,Y)>-m(fb(D))}
# I
where m(fi(D))=-ti(x. y)dxdy area(D)
We will consider the following vectors:
v,=(m(f,(D)),m(f, (D))?m(fb(D)))
# vI=(m(f, (D rl )).m(fg (Dgi)km(fb(Dbi
V z =(m(f,(D,,)),m(f, (D,z )),m(fb (DM )))
uL=(th(l)th(l)th(l))
### wo=.( 0, 0, O),w,=(-0.5,-0.5,-0.5),
w 2 =(0.5,0.5,0.5)
### 4.1 The algorithm A
( 1 1 )
We obtain the parameters a.0 from the overdetermined linear system:
# aW v , (+)P(x)ur =w o
0 2 )
a(x)(vz(-)vo)=wz

---

## Page 3

### -c,-c,
### cw%-(c,P
$$ P = $$
andthe transformation is:
and the minimum mean square error solutionis:
C w ' C n
## a=
P= (19)
### CW%"-(c,P
and the transformation is:
c"c,,
t(f)= (x)
cW.c""-(LP
## 5. EXPERIMENTAL RESULTS
We exemplify our algorithms on more of less standard test images: "kidsaw(Fig. I ) and "boat" (Fig. 4). Forthe "kidsaw image we get the following values: vo=(-0.694. 4.727, -0.580), VI=(-0.868,-0.846, -0.714). v2=(-0.347, -0.S15, -0.459).
## ,135
Figure I . Original image "kidsatl"
Figure2. Enhanced image ("kidsaU") with algorithmA
Figure3. Enhancedimage ("kidsad") with algorithm B

---

## Page 4

Figure4. Original image “boat”
Figure 5. Enhanced’image (“boat”) with algorithm A
Figure 6 . Enhanced’image (“boat”) with algorithm B
The algorithmA gives: a =1.384,p=1.116, k=(0.762, 0.762, 0.762), t(f)=l.384(x)f(+)l.116(x)k.ThealgorithmBgives:
## a=1.450,p=-I.447,
k=(-0.694, -0.727, -0.580),
## t(f)=1.450(x)f(-)I.447(x)k.
Enhanced images for “kidsat3” are showed in Fig. 2 for algorithm A and in Fig. 3 for algorithm B. For the “boat” image we obtained the following values: v0<-0.194, -0.323, -0.338), v,=(-0.557, -0.675, -0.676) , v2=(-0.001. -0.119, -0,107). T h e algorithm A gives:
## a=1.402 !3=0.412,
k=(0.762, 0.762, 0.762).
## t(f)=1.402(x)f(+)0.412(x)k. The algorithm B gives:
## a=1.538,p=-l.941,
k=(-0.194, :0.323, -0.338), t(f)=1.538(x)f(-)1.941(x)k. Enhanced images for “boat” are showed in Fig. 5 for algorithm A and in Fig. 6 for algorithm B.
## 6. CONCLUSIONS
color space that we present is very useful for image processing: it allows one to quickly define a large set of algorithms. The main advantage consists in the ability to parameterize the algorithms and to simply introduce optimality criteria. On the other hand, the computing involved is kept at the lowest level (a lineartransformation for each pixel).
## 7. REFERENCES
Images. In Proc. of the 71h International Conference, Exhibition on Optimization of Electrical and Electronic Equipment, OPTIM 2000. pp. 809-812. Brwav, Romania, 11-12May 2000. [6] A. Rosenfeld, A.C. Kak, Digirol Picrure Processing. Academic Press, New York, 1982. [7] T.G. Srockham. Image processing in rhe conre~lof visual model. Proc. IEEE ,Vol. 60, July 1972.