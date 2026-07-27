<!-- Converted from florea2008.pdf — 6 pages -->

## Page 1

## Isse 2008, Galway, June 18-19
## Logarithmic Tools for In-camera Image
## Processing
## Corneliu Florea', Felix Albu', Constantin Vertan" Alexandru
## Drimbarean..
*FOloNation Romania ··FotoNation Ireland
email ·{cornel.!elLy..cverton}@folonolion.com email: alexd@fOfonotiOll.com
process color Images and show how they can be used to implement low-light image enhancement in digital cameras.
Keywortls- Logarithmic Models, Image processing, Low-light, Digital Camera
The image is captured with an adequate exposure
common way of acquiring images. Nowadays, the main direction seems to be that ofdecreasing the size
tremendous efforts in the Megapixels race. achieved in digital domain by means of de-
the size of optics and of photosensitive area. If we
## discuss these issues from an end-user point of view,
the problem is that of increased susceptibility of
is imposing design modifications such as reducing convolution. However this alternative implies the use
## of motion sensors (which came as an extra circuit)
with attempts to deal with it. We may divide these

---

## Page 2

the low-light enhancement. The paper ends with a
## discussion of the obtained results, conclusions and
future possible work.
## II
The classical logarithmic model is generated via a basic isomorphic transform 't' that maps the image value definition domain (typically denoted as the interval [0, MD, into the logarithmic space:
## -h)
## 'I'(h) = M(1-exp-M (I)
The inverse mapping is defined by:
(2)
law, as defined in equations (3) and (4) below:
## f al g = f +g - ( 2)
(3)
## a<8>f=M-M(I- ~r
(4)
range.
.•...
| ,,. | .,,~.....•. |
|---|---|
|  | / |
## /
| '. | ". | ,,. |
|---|---|---|
| Figure |   1:   eRF   to   Log   | Amplification |
| typical   consumer |   camera   eRF   (solid |   line)   |
|  |   with   a   =   | 1.78   (doued |
experimentally derived CRF provides a more accurate result for a given digital camera. Still, the
## use of the LIP multiplication provides more
## generality (which means that brings the advantage of
manufacturers.

---

## Page 3

(d) (e) (f) Figure 2. (a) Example oflow-light main image, (b) reference, motion blurred image, ~c) ideal image, (d) logarithmic enhanced image, (e) linear enhanced image, (f) auto-contrast Image.
equation proposed solution, an important demand is the
# relative short period between the capture moments of
$$ E =-log (t)+2Iog =(<I>~) $$
(5) v 2 2the two required images: the reference and the main image. This helps prevent large geometrical where v is the exposure value, the log ofI forms the misalignments and, hence, avoid the time-consuming time value (Tv), N is the relative diaphragm opening image registration (as is the case of[12]).
### (by taking the log, the aperture value, Av, is formed),
which is of N x P resolution with F(i,j), i=1, ... N, for digital cameras - the amplification) and K is a
### j=1, .. P. The reference image (which we shall denote
known constant. If the aperture, which has influence by G(i,j)) may have the same or lower resolution
# over the depth of field, is held constant, while the
# than the subject, image. If different resolutions are
incident light is fixed, then there is a direct relation
# used one maya use a nearest neighbour interpolation
between the exposure time and the sensitivity. For to even the image sizes (to the high resolution (N x older film-cameras, sensitivity was given by the P)).
# amount of photo-sensible particles per square unit. If
The enhancement method works in two steps; a larger density is used then a shorter time is required first, it performs a rough global amplification; later,
# to capture the number of particles that change their
paragraphs, we shall proceed with describing the
# replaced by high ISO mode, where the S parameter
A typical low-light enhancement method amplification is performed by considering a reduced firstly captures an image with a short exposure time.
# set of (spatially matching) pixels from each image:
This image is hand motion free but under-exposed. G c G and, respectively F, c F, and by means of Next, the image is amplified until its luminance and
# colour levels match that ofa reference. The reference
or it may be an external one, (other image as described in [12]).
(cland C2), [13], so that:
(6)

---

## Page 4

so that: as:
F2(i,) =W(i,)~(i,) (7) lVlk)
F (k)=D-D 1_F;~k) (10)
2  
(  
)
without loss in image quality. the 2-D images are turned into l-D vectors by arranging them in lexicographic order:
### IV
I consumer camera held in the hands; thus they are susceptible to motion blur. The under-exposed W(k+I)=W(k)+,u(k)·e(k) (8) subject image (Fig. 2, (a)) was obtained by forcing the exposure value to be Ev=-2. The ideal image where p(k) is the step size (fixed or variable), the (which is not affected by motion blur) was recorded e(k) is the error signal computed as: with a tripod mounted camera. As results, we showed our image (d), the image obtained by
e(k)=G(k)-F (k) (9)
2described amplification but performed in a linear space (e) (which is obviously over saturated in the

---

## Page 5

poor colour reproduction. Concluding, it is easy to observe that the best results are obtained by using the two step logarithmic amplification. Furthermore, we considered an extensive testing procedure, with three scenarios (our method with logarithmic and respectively linear amplification which includes larger data sets, with diff! r nt scenes, but under the constraint that th r is no misalignment between the ideal image and the resulting ones. Under these circumstances, we were able to compute a ormalized Mean Square Error (NMSE). NMSE is an estimator of the overall deviation between the ideal image I and measured image M and is computed according to the formula below:
MSE=~L(Ji-Mif
### (II)
1M
results in all conditions.
|  | LOG | LIN | AUTO |
|---|---|---|---|
| EV=-l | 0.0053 | 0.0214 | 0.0120 |
| EV=-2 | 0.0063 | 0.0254 | 0.0182 |
LOG LIN EV=-l 0.0053 0.0214 0.0254
|  |  |
|---|---|
| Table   I.   Values of |   the   |
| amplification   (LOG), |   |
| contrast   (AUTO)   if   | the   low-light   |
| with   | EV=I·   or   EV=·2 |
| The   method |   shows   |
|   of   | underexposed images with more than EV=-2 |
|  | (Figure. 3 illustrates poor colour reproduction |
| case)   or   sizeable |   misalignment |
| images (our test showed a value |  |
| 5Mpixel   image   | as   the   |
| misalignment).   | Under such conditions even |
| method   is   | the most reliable,   it   |
| inspection. |  |
|  | As described, the method |
| software   | implementation   and   |
| processors   can   | handle   the   |
| implementation   it |   is   possible,   |
| simplicity, the cost |   of   |
|  | may not compensated by speed improvement. |
### V
CONCLUSIO S In this paper we proposed a method that makes use of the similarity between digital still camera response function and the LIP multiplication to derive a technique for low-light images
easily implemented in existing digital camera and mobile phone embedded platforms and gives better results than other known solutions.
D. Van De Ville, E.E. Kerre, (Series Studies in Fuzziness and Soft Computing, Vol. 122) Physica Verlag, 2003, pp. 219-237. [7] P. Debevec and J. Malik, 'Recovering
high dynamic range radiance maps from
photographs",   Proc.   of   ACM   SIGGRAPH   24th
Annual   Conference   on   Computer   Graphics   and
Interactive   Techniques   Vol. I,   Los   Angeles   CA,
USA, 1997, pp 369-378.
[8]   M. Jourlin and J.C. Pinoli "Justifications
physiques   et applications   du   modele   LIP   pour   Ie
traitement   des   images obtenues   en   lumiere
transmise "   Traitement   du   Signal,   vol. 3,   )996, pp
251-262.
[9]   v.c.   Chang   and   J.   F.   Reid,   "RGB
Calibration   for   Color   Image   Analysis   in   Machine
Vision",   IEEE Transactions on Image Processing,
Vol. 5, October 1996, pp 1414-1422.
[ I0]   S.   K.   ayar and   T.   Mitsunaga,   "H igh
Dynamic   Range   Imaging: Spatially   Varying   Pixel
Exposures", Proc.   of   IEEE Conference on Computer
Vision and Pattern Recognition,   Vol.   I   June 2000,
pp.472-479.
[I   I]   S.   Mann   and   R.   Picard,   "Being
'Undigital'   with   Digital   Cameras:   Extending
Dynamic Range by Combininr, Differently Exposed
Pictures",   Proc.   of   1ST's   48/1   Annual Conference,
May 1995, pp. 422-428.

---

## Page 6

Baltimore, MD, 3rdedition, 1996